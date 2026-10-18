#!/usr/bin/env python3
"""Test-only durable journal response barrier. Never changes production clients."""
import http.client
import http.server
import json
import os
from pathlib import Path
import re
import select
import subprocess
import tempfile
import threading
import urllib.parse


class JournalProxy:
    """Forward REST calls, holding one committed Applying response until release."""
    def __init__(self, upstream, namespace, installation, target):
        self.upstream=urllib.parse.urlsplit(upstream)
        assert self.upstream.scheme=='http' and self.upstream.hostname=='127.0.0.1'
        self.collection='/api/v1/namespaces/'+namespace+'/configmaps'
        self.name=installation+'-operation';self.target=target
        self.applied=threading.Event();self.release=threading.Event()
        owner=self
        class Handler(http.server.BaseHTTPRequestHandler):
            protocol_version='HTTP/1.1'
            def log_message(self,*args):pass
            def forward(self):
                connection=None
                try:
                    size=int(self.headers.get('Content-Length','0'))
                    if not 0<=size<=32*1024*1024:raise ValueError('InvalidFixtureRequestSize')
                    body=self.rfile.read(size) if size else b''
                    headers={k:v for k,v in self.headers.items() if k.lower() not in {'host','connection','transfer-encoding','content-length'}}
                    headers['Connection']='close'
                    connection=http.client.HTTPConnection(owner.upstream.hostname,owner.upstream.port,timeout=120)
                    connection.request(self.command,self.path,body=body,headers=headers)
                    response=connection.getresponse();payload=response.read(64*1024*1024+1)
                    if len(payload)>64*1024*1024:raise ValueError('InvalidFixtureResponseSize')
                    if 200<=response.status<300 and owner.matches(self.command,self.path,body):
                        # The upstream write is complete. Withholding the response
                        # keeps the CLI awaiting save(), so it cannot finish early.
                        owner.applied.set()
                        if not owner.release.wait(180):raise TimeoutError('JournalBarrierTimedOut')
                    self.send_response(response.status)
                    for key,value in response.getheaders():
                        if key.lower() not in {'connection','transfer-encoding','content-length'}:self.send_header(key,value)
                    self.send_header('Content-Length',str(len(payload)));self.send_header('Connection','close');self.end_headers();self.wfile.write(payload)
                except (BrokenPipeError,ConnectionResetError):pass
                except Exception:
                    # Do not log request bodies, credentials or upstream errors.
                    try:self.send_error(502,'FixtureTransportFailed')
                    except (BrokenPipeError,ConnectionResetError):pass
                finally:
                    if connection:connection.close()
            do_GET=do_POST=do_PUT=do_PATCH=do_DELETE=forward
        self.server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler)
        self.server.daemon_threads=True
        self.thread=threading.Thread(target=self.server.serve_forever,daemon=True);self.thread.start()
        self.url='http://127.0.0.1:'+str(self.server.server_port)
    def matches(self,method,path,body):
        path=urllib.parse.urlsplit(path).path
        if method not in {'POST','PUT','PATCH'} or path not in {self.collection,self.collection+'/'+self.name}:return False
        try:
            value=json.loads(body)
            if value.get('metadata',{}).get('name')!=self.name:return False
            record=json.loads(value['data']['operation.json'])
            return record['phase']=='Applying' and record['target']['release']==self.target
        except (ValueError,KeyError,TypeError,AttributeError):return False
    def close(self):
        self.release.set();self.server.shutdown();self.server.server_close();self.thread.join(timeout=3)


class Client:
    """Use kubectl's authenticated, TLS-verified upstream with a private config."""
    def __init__(self,command,namespace,installation,target,environment):
        self.command=command;self.namespace=namespace;self.installation=installation
        self.target=target;self.original_environment=environment
        self.directory=None;self.process=None;self.proxy=None
    def __enter__(self):
        try:
            self.directory=tempfile.TemporaryDirectory(prefix='kimage-journal-barrier-')
            directory=Path(self.directory.name);directory.chmod(0o700)
            self.errors=(directory/'kubectl.private.log').open('w')
            self.process=subprocess.Popen([*self.command,'proxy','--address=127.0.0.1','--port=0'],env=self.original_environment,stdout=subprocess.PIPE,stderr=self.errors,text=True)
            assert select.select([self.process.stdout],[],[],30)[0],'FixtureApiProxyStartupTimedOut'
            line=self.process.stdout.readline();match=re.fullmatch(r'Starting to serve on 127\.0\.0\.1:(\d+)\s*',line)
            assert match and self.process.poll() is None,'FixtureApiProxyStartupFailed'
            self.proxy=JournalProxy('http://127.0.0.1:'+match[1],self.namespace,self.installation,self.target)
            path=directory/'kubeconfig.json'
            value=dict(apiVersion='v1',kind='Config',clusters=[dict(name='fixture',cluster=dict(server=self.proxy.url))],contexts=[dict(name='fixture',context=dict(cluster='fixture',user='fixture',namespace=self.namespace))],users=[dict(name='fixture',user={})],**{'current-context':'fixture'})
            descriptor=os.open(path,os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600)
            with os.fdopen(descriptor,'w') as output:json.dump(value,output)
            self.environment=dict(self.original_environment,KUBECONFIG=str(path))
            self.applied=self.proxy.applied
            return self
        except BaseException:
            self.__exit__(None,None,None);raise
    def __exit__(self,*args):
        if self.proxy:self.proxy.close()
        if self.process:
            self.process.terminate()
            try:self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:self.process.kill();self.process.wait(timeout=10)
            if self.process.stdout:self.process.stdout.close()
        if hasattr(self,'errors'):self.errors.close()
        if self.directory:self.directory.cleanup()

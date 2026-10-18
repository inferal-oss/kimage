#!/usr/bin/env python3
"""Prove durable writes precede the interruption barrier, without timed polling."""
import http.client
import http.server
import importlib.util
import io
import json
import os
from pathlib import Path
import threading
import unittest
from unittest.mock import patch

ROOT=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location('interruption',ROOT/'scripts/e2e-interruption.py')
interruption=importlib.util.module_from_spec(spec);spec.loader.exec_module(interruption)

class BarrierTests(unittest.TestCase):
    def setUp(self):
        owner=self;self.saved=None;self.status=200
        class Upstream(http.server.BaseHTTPRequestHandler):
            def log_message(self,*args):pass
            def do_PUT(self):
                owner.saved=json.loads(self.rfile.read(int(self.headers['Content-Length'])))
                body=json.dumps(owner.saved).encode();self.send_response(owner.status);self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
            def do_GET(self):
                body=json.dumps(owner.saved).encode();self.send_response(200);self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
        self.upstream=http.server.ThreadingHTTPServer(('127.0.0.1',0),Upstream)
        self.thread=threading.Thread(target=self.upstream.serve_forever,daemon=True);self.thread.start()
        self.proxy=interruption.JournalProxy('http://127.0.0.1:'+str(self.upstream.server_port),'system','fixture','0.6.1')
        self.path='/api/v1/namespaces/system/configmaps/fixture-operation'
    def tearDown(self):
        self.proxy.close();self.upstream.shutdown();self.upstream.server_close();self.thread.join(timeout=3)
    def body(self,phase='Applying',target='0.6.1',name='fixture-operation'):
        return dict(metadata=dict(name=name),data={'operation.json':json.dumps(dict(phase=phase,target=dict(release=target)))})
    def request(self,body,results):
        connection=http.client.HTTPConnection('127.0.0.1',self.proxy.server.server_port,timeout=5)
        try:
            connection.request('PUT',self.path,json.dumps(body),{'Content-Type':'application/json'})
            response=connection.getresponse();results.append((response.status,response.read()))
        finally:connection.close()
    def test_committed_journal_is_readable_while_writer_cannot_advance(self):
        results=[];writer=threading.Thread(target=self.request,args=(self.body(),results));writer.start()
        try:
            self.assertTrue(self.proxy.applied.wait(3));self.assertEqual(self.saved,self.body());self.assertFalse(results)
            connection=http.client.HTTPConnection('127.0.0.1',self.proxy.server.server_port,timeout=3)
            connection.request('GET',self.path);response=connection.getresponse();self.assertEqual(json.loads(response.read()),self.body());connection.close()
            self.assertFalse(results,'Writer must still be awaiting its response after the independent read')
        finally:self.proxy.release.set();writer.join(timeout=5)
        self.assertFalse(writer.is_alive());self.assertEqual(results[0][0],200)
    def test_other_phases_targets_and_resources_are_forwarded_without_barrier(self):
        for body in [self.body(phase='Complete'),self.body(target='0.6.0'),self.body(name='unrelated')]:
            results=[];self.request(body,results);self.assertEqual(results[0][0],200);self.assertFalse(self.proxy.applied.is_set())
    def test_rejected_write_cannot_be_reported_as_durable(self):
        self.status=409;results=[];self.request(self.body(),results)
        self.assertEqual(results[0][0],409);self.assertFalse(self.proxy.applied.is_set())
    def test_private_client_configuration_is_temporary_and_original_environment_is_unchanged(self):
        class Process:
            def __init__(inner,*args,**kwargs):
                inner.stdout=io.StringIO('Starting to serve on 127.0.0.1:'+str(self.upstream.server_port)+'\n');inner.stopped=False
            def poll(inner):return None
            def terminate(inner):inner.stopped=True
            def wait(inner,timeout):return 0
        environment=dict(KUBECONFIG='original-private-path')
        with patch.object(interruption.subprocess,'Popen',Process),patch.object(interruption.select,'select',side_effect=lambda read,*args:(read,[],[])):
            with interruption.Client(['kubectl','--context','fixture'],'system','fixture','0.6.1',environment) as client:
                path=Path(client.environment['KUBECONFIG']);self.assertEqual(path.stat().st_mode & 0o777,0o600)
                config=json.loads(path.read_text());self.assertEqual(config['clusters'][0]['cluster']['server'],client.proxy.url)
                self.assertEqual(config['users'][0]['user'],{});self.assertEqual(environment,dict(KUBECONFIG='original-private-path'))
            self.assertFalse(path.exists());self.assertTrue(client.process.stopped)

if __name__=='__main__':unittest.main()

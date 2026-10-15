#!/usr/bin/env python3
"""Guarded payload cleanup: active mounts, foreign bytes and idempotent resume."""
import argparse
import base64
import json
import os
import pathlib
import re
import subprocess
import time

p=argparse.ArgumentParser(description=__doc__)
for name in ['kubeconfig','options','installer','bundle','results']:p.add_argument('--'+name,type=pathlib.Path,required=True)
p.add_argument('--trusted-inventory',required=True)
a=p.parse_args()
assert re.search(r'server: https://127\.0\.0\.1:27643\s',a.kubeconfig.read_text())
o=json.loads(a.options.read_text());assert (o['namespace'],o['installation'])==('kimage-system','kimage-clean')
k=['kubectl','--kubeconfig',str(a.kubeconfig),'--request-timeout=30s','-n',o['namespace']]
env=dict(os.environ,KUBECONFIG=str(a.kubeconfig.resolve()))
for key in ['KIMAGE_API_SERVER','KIMAGE_TLS_SERVER_NAME']:env.pop(key,None)
def call(args,body=None):
    result=subprocess.run(k+args,input=None if body is None else json.dumps(body).encode(),capture_output=True,timeout=120)
    if result.returncode:raise RuntimeError(result.stderr.decode()[:1000])
    return result.stdout
def get(kind,name):return json.loads(call(['get',kind,name,'-o','json']))
def create(body):return json.loads(call(['create','-f','-','-o','json'],body))
def delete(kind,value):
    plural={'Pod':'pods','ConfigMap':'configmaps'}[kind]
    path='/api/v1/namespaces/'+o['namespace']+'/'+plural+'/'+value['metadata']['name']
    call(['delete','--raw',path,'-f','-'],dict(apiVersion='v1',kind='DeleteOptions',gracePeriodSeconds=1,preconditions=dict(uid=value['metadata']['uid'],resourceVersion=value['metadata']['resourceVersion'])))
def cleanup(reason=None):
    args=[]
    for key,value in o.items():args+=['--'+''.join('-'+c.lower() if c.isupper() else c for c in key),str(value)]
    r=subprocess.run([str(a.installer.resolve()),'cleanup']+args+['--bundle',str(a.bundle.resolve()),'--trusted-inventory',a.trusted_inventory],env=env,capture_output=True,timeout=180)
    if reason:assert r.returncode and reason.encode() in r.stderr,r.stderr.decode()[:500]
    else:assert r.returncode==0,r.stderr.decode()[:500]
labels={'kimage.dev/installation':o['installation']}
name=o['installation']+'-launcher'
inventory=json.loads((a.bundle/'inventory.json').read_text())
launcher=(a.bundle/inventory['launcher']['file']).read_bytes()
cm=create(dict(apiVersion='v1',kind='ConfigMap',metadata=dict(name=name,labels=labels),immutable=True,binaryData=dict(launcher=base64.b64encode(launcher).decode())))
pod=create(dict(apiVersion='v1',kind='Pod',metadata=dict(name=o['installation']+'-cleanup-guard',labels=labels),spec=dict(automountServiceAccountToken=False,nodeName=o['node'],restartPolicy='Never',containers=[dict(name='holder',image=o['carrier'],imagePullPolicy='Never',resources=dict(requests=dict(cpu='10m',memory='8Mi'),limits=dict(cpu='100m',memory='32Mi')),securityContext=dict(allowPrivilegeEscalation=False,readOnlyRootFilesystem=True,capabilities=dict(drop=['ALL'])),volumeMounts=[dict(name='payload',mountPath='/payload',readOnly=True)])],volumes=[dict(name='payload',configMap=dict(name=name))])))
try:
    cleanup('BootstrapPayloadStillInUse')
    assert get('configmap',name)['metadata']['uid']==cm['metadata']['uid']
finally:
    current=get('pod',pod['metadata']['name']);delete('Pod',current)
    deadline=time.monotonic()+60
    while time.monotonic()<deadline:
        r=subprocess.run(k+['get','pod',pod['metadata']['name'],'-o','name'],capture_output=True)
        if r.returncode and b'NotFound' in r.stderr:break
        time.sleep(0.5)
    else:raise RuntimeError('cleanup guard Pod termination timeout')
cleanup()
name=o['installation']+'-registry-0000'
conflict=create(dict(apiVersion='v1',kind='ConfigMap',metadata=dict(name=name,labels=labels),immutable=True,binaryData={'0000':base64.b64encode(b'incorrect fixture payload').decode()}))
try:
    cleanup('BootstrapCleanupPayloadConflict')
    current=get('configmap',name)
    assert all(current['metadata'][key]==conflict['metadata'][key] for key in ['uid','resourceVersion'])
finally:
    delete('ConfigMap',get('configmap',name))
cleanup();cleanup()
remaining=json.loads(call(['get','configmaps','-o','json']))['items']
assert not any(re.fullmatch(r'kimage-clean-(launcher|(?:registry|gateway|proxy)-[0-9]{4})',v['metadata']['name']) for v in remaining)
a.results.write_text(json.dumps(dict(activePayloadMountRefused=True,conflictingPayloadPreserved=True,ownedPayloadRemovedAfterUnmount=True,idempotentRepeat=True,noBootstrapExecutableConfigMapsRemain=True),indent=2)+'\n')
print('guarded bootstrap cleanup and idempotent repeat passed',flush=True)

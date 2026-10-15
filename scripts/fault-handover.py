#!/usr/bin/env python3
"""Interrupt one test-owned handover after its durable checkpoint, then resume."""
import argparse
import hashlib
import json
import os
import pathlib
import re
import subprocess
import time
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--options',type=pathlib.Path,required=True)
p.add_argument('--bundle',type=pathlib.Path,required=True)
p.add_argument('--kubeconfig',type=pathlib.Path)
p.add_argument('--installer',type=pathlib.Path,default=pathlib.Path('target/debug/kimage-install'))
a=p.parse_args();o=json.loads(a.options.read_text())
if a.kubeconfig:
 assert re.search(r'server: https://127\.0\.0\.1:[0-9]+\s',a.kubeconfig.read_text())
 assert o['runtimeProfile']=='k3s' and o['namespace'].startswith('kimage-')
 os.environ['KUBECONFIG']=str(a.kubeconfig.resolve())
 for key in ['KIMAGE_API_SERVER','KIMAGE_TLS_SERVER_NAME']:os.environ.pop(key,None)
 k=['kubectl','--kubeconfig',str(a.kubeconfig),'--request-timeout=10s','-n',o['namespace']]
else:
 assert o['namespace']=='dev-yrashk' and o['installation'].startswith('kimage-foundation-')
 assert subprocess.check_output(['kubectl','config','current-context'],text=True).strip()=='inferal'
 k=['kubectl','--request-timeout=10s','-n',o['namespace']]
 if os.environ.get('KIMAGE_API_SERVER'):
  k+=['--server',os.environ['KIMAGE_API_SERVER'],'--tls-server-name',os.environ['KIMAGE_TLS_SERVER_NAME']]
args=[str(a.installer),'handover']
for key,flag in [('namespace','namespace'),('installation','installation'),('node','node'),('carrier','carrier'),('storageClass','storage-class'),('port','port'),('credentials','credentials')]:args+=['--'+flag,str(o[key])]
if o.get('applicationNamespace'):args+=['--application-namespace',o['applicationNamespace']]
if o.get('runtimeProfile'):args+=['--runtime-profile',o['runtimeProfile']]
args+=['--bundle',str(a.bundle),'--trusted-inventory','sha256:'+hashlib.sha256((a.bundle/'inventory.json').read_bytes()).hexdigest()]
name=o['installation']+'-registry'
process=subprocess.Popen(args,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
interrupted=False
try:
 deadline=time.monotonic()+240
 while time.monotonic()<deadline:
  if process.poll() is not None:raise RuntimeError('handover completed before the fault window; interruption not proven')
  checkpoint=subprocess.run(k+['get','cm',o['installation']+'-replacement-registry','-o','json'],stdout=subprocess.PIPE,stderr=subprocess.DEVNULL)
  pod=subprocess.run(k+['get','pod',name,'-o','json'],stdout=subprocess.PIPE,stderr=subprocess.DEVNULL)
  if checkpoint.returncode==0 and (pod.returncode!=0 or json.loads(pod.stdout)['metadata'].get('deletionTimestamp')):
   record=json.loads(checkpoint.stdout)
   assert record['metadata']['labels']['kimage.dev/installation']==o['installation']
   process.terminate();process.communicate(timeout=10);interrupted=True;break
  time.sleep(0.2)
 assert interrupted,'fault window not observed'
 deadline=time.monotonic()+90
 while time.monotonic()<deadline:
  result=subprocess.run(k+['get','pod',name,'-o','name'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
  if result.returncode:break
  time.sleep(0.5)
 else:raise RuntimeError('old serving Pod did not terminate')
 subprocess.run(args,check=True)
 (a.options.parent/'handover-interruption-evidence.json').write_text(json.dumps(dict(checkpointBeforeDeletion=True,installerTerminated=True,registryAbsentOnResume=True,resumedOrdinaryServing=True)))
 print('interrupted single-writer handover resumed from its durable checkpoint')
finally:
 if process.poll() is None:process.terminate();process.communicate(timeout=10)

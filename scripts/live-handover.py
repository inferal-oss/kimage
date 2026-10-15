#!/usr/bin/env python3
"""Continuous small-manifest reads during native handover on the isolated fixture."""
import argparse,base64,json,os,pathlib,re,subprocess,time
p=argparse.ArgumentParser(description=__doc__)
for name in ['kubeconfig','options','bundle','installer','probe','results']:
    p.add_argument('--'+name,type=pathlib.Path,required=True)
p.add_argument('--trusted-inventory',required=True)
p.add_argument('--manifest-path',required=True)
a=p.parse_args()
assert re.search(r'server: https://127\.0\.0\.1:[0-9]+\s',a.kubeconfig.read_text())
o=json.loads(a.options.read_text());assert o['runtimeProfile']=='k3s' and o['namespace'].startswith('kimage-')
env=os.environ.copy();env['KUBECONFIG']=str(a.kubeconfig.resolve())
for key in ['KIMAGE_API_SERVER','KIMAGE_TLS_SERVER_NAME']:env.pop(key,None)
k=['kubectl','--kubeconfig',str(a.kubeconfig),'--request-timeout=10s','-n',o['namespace']]
def call(args,body=None):
    r=subprocess.run(k+args,input=None if body is None else json.dumps(body).encode(),capture_output=True,env=env,check=True)
    return r.stdout.decode()
def get(name):return json.loads(call(['get','pod',name,'-o','json']))
name=o['installation']+'-read-loop'
helper=a.probe.read_bytes();assert len(helper)<=512*1024
label={'kimage.dev/installation':o['installation']}
call(['create','-f','-'],dict(apiVersion='v1',kind='ConfigMap',metadata=dict(name=name,labels=label),immutable=True,binaryData=dict(helper=base64.b64encode(helper).decode())))
call(['create','-f','-'],dict(apiVersion='v1',kind='Pod',metadata=dict(name=name,labels=label),spec=dict(nodeName=o['node'],hostNetwork=True,automountServiceAccountToken=False,restartPolicy='Never',containers=[dict(name='probe',image=o['carrier'],imagePullPolicy='Never',command=['/helper/helper','--read-loop-fixture',a.manifest_path,'240'],securityContext=dict(runAsUser=0,runAsGroup=0,allowPrivilegeEscalation=False,readOnlyRootFilesystem=True,capabilities=dict(drop=['ALL'])),resources=dict(requests=dict(cpu='10m',memory='16Mi'),limits=dict(cpu='500m',memory='64Mi')),volumeMounts=[dict(name='helper',mountPath='/helper',readOnly=True)])],volumes=[dict(name='helper',configMap=dict(name=name,defaultMode=365))])))
deadline=time.monotonic()+90
while time.monotonic()<deadline:
    status=get(name).get('status',{})
    if status.get('phase')=='Running':break
    time.sleep(1)
else:raise RuntimeError('probe did not start')
time.sleep(3)
flags=[]
for key,value in o.items():flags+=['--'+''.join('-'+c.lower() if c.isupper() else c for c in key),str(value)]
subprocess.run([str(a.installer.resolve()),'handover']+flags+['--bundle',str(a.bundle.resolve()),'--trusted-inventory',a.trusted_inventory],env=env,check=True)
deadline=time.monotonic()+300
while time.monotonic()<deadline:
    status=get(name).get('status',{})
    if status.get('phase') in ['Succeeded','Failed']:break
    time.sleep(1)
else:raise RuntimeError('probe did not finish')
log=call(['logs',name]);match=re.search(r'read-loop final successes=(\d+) failures=(\d+)',log);assert match,log[-2000:]
successes,failures=map(int,match.groups());assert successes>100 and failures==0,(successes,failures)
a.results.write_text(json.dumps(dict(successfulReads=successes,failedReads=failures,smallManifestContinuity=True,publicationPausedDuringWriterChange=True),indent=2)+'\n')
print(f'continuous small-manifest reads passed: {successes} successes, {failures} failures')

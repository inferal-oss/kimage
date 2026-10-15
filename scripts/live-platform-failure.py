#!/usr/bin/env python3
"""Deploy a development operator digest and recheck the retained invalid input.

This explicit test replacement is not the product self-upgrade mechanism.
"""
import argparse,json,os,pathlib,re,subprocess,time
p=argparse.ArgumentParser(description=__doc__)
for name in ['kubeconfig','options','installer','bundle','results']:p.add_argument('--'+name,type=pathlib.Path,required=True)
p.add_argument('--trusted-inventory',required=True)
p.add_argument('--image',required=True)
a=p.parse_args();assert re.search(r'server: https://127\.0\.0\.1:27643\s',a.kubeconfig.read_text())
o=json.loads(a.options.read_text());assert (o['namespace'],o['applicationNamespace'],o['installation'])==('kimage-system','kimage-app','kimage-clean')
assert re.fullmatch(r'kimage-clean-bad-platform-[0-9a-f]{8}',a.image)
k=['kubectl','--kubeconfig',str(a.kubeconfig),'--request-timeout=30s']
def call(args,body=None):
    r=subprocess.run(k+args,input=None if body is None else json.dumps(body).encode(),capture_output=True,timeout=120)
    if r.returncode:raise RuntimeError(r.stderr.decode()[:1500])
    return r.stdout
def get(kind,name,ns):return json.loads(call(['-n',ns,'get',kind,name,'-o','json']))
deadline=time.monotonic()+240
while time.monotonic()<deadline:
    r=subprocess.run(k+['get','--raw=/readyz'],capture_output=True,timeout=45)
    if r.returncode==0 and r.stdout.strip()==b'ok':
        serving=[get('pod','kimage-clean-'+role,o['namespace']) for role in ['registry','proxy']]
        if all(any(c['type']=='Ready' and c['status']=='True' for c in pod.get('status',{}).get('conditions',[])) for pod in serving):break
    time.sleep(1)
else:raise RuntimeError('owned fixture API/serving readiness timeout')
args=[]
for key,value in o.items():args+=['--'+''.join('-'+c.lower() if c.isupper() else c for c in key),str(value)]
env=dict(os.environ,KUBECONFIG=str(a.kubeconfig.resolve()))
for key in ['KIMAGE_API_SERVER','KIMAGE_TLS_SERVER_NAME']:env.pop(key,None)
inventory=json.loads((a.bundle/'inventory.json').read_text())
reference='127.0.0.1:17641/kimage-system/system-operator@'+inventory['components']['operator']['root']['digest']
pod=get('pod','kimage-clean-operator',o['namespace'])
if pod['spec']['containers'][0]['image']!=reference:
    subprocess.run([str(a.installer.resolve()),'seed']+args+['--bundle',str(a.bundle.resolve()),'--trusted-inventory',a.trusted_inventory],env=env,check=True,timeout=600)
else:
    subprocess.run([str(a.installer.resolve()),'bundle-check','--bundle',str(a.bundle.resolve()),'--trusted-inventory',a.trusted_inventory],check=True,timeout=120)
assert pod['metadata']['labels']['kimage.dev/installation']==o['installation'] and len(pod['spec']['containers'])==1
if pod['spec']['containers'][0]['image']!=reference:
    replacement=dict(apiVersion='v1',kind='Pod',metadata=dict(name=pod['metadata']['name'],labels=pod['metadata']['labels']),spec=pod['spec'])
    replacement['spec']['containers'][0]['image']=reference
    body=dict(apiVersion='v1',kind='DeleteOptions',gracePeriodSeconds=1,preconditions=dict(uid=pod['metadata']['uid'],resourceVersion=pod['metadata']['resourceVersion']))
    call(['delete','--raw','/api/v1/namespaces/'+o['namespace']+'/pods/'+pod['metadata']['name'],'-f','-'],body)
    deadline=time.monotonic()+90
    while time.monotonic()<deadline:
        r=subprocess.run(k+['-n',o['namespace'],'get','pod',pod['metadata']['name']],capture_output=True)
        if r.returncode and b'NotFound' in r.stderr:break
        time.sleep(0.5)
    else:raise RuntimeError('operator termination timeout')
    call(['-n',o['namespace'],'create','-f','-'],replacement)
deadline=time.monotonic()+240
while time.monotonic()<deadline:
    pod=get('pod','kimage-clean-operator',o['namespace'])
    if any(c['type']=='Ready' and c['status']=='True' for c in pod.get('status',{}).get('conditions',[])):break
    time.sleep(1)
else:raise RuntimeError('replacement operator readiness timeout')
image=get('image',a.image,o['applicationNamespace'])
patch=dict(metadata=dict(uid=image['metadata']['uid'],resourceVersion=image['metadata']['resourceVersion']),spec=dict(channels=dict(recheck='1.0.0')))
call(['-n',o['applicationNamespace'],'patch','image',a.image,'--type=merge','-p',json.dumps(patch)])
deadline=time.monotonic()+180
while time.monotonic()<deadline:
    image=get('image',a.image,o['applicationNamespace']);status=image.get('status',{})
    assert not any(c['type']=='Ready' and c['status']=='True' for c in status.get('conditions',[]))
    if status.get('observedGeneration')==image['metadata']['generation'] and status.get('operation',{}).get('phase')=='Failed' and status['operation']['identity']['generation']==image['metadata']['generation']:break
    time.sleep(1)
else:raise RuntimeError('wrong platform did not become a permanent failure')
assert status['conditions'][0]['reason']=='WrongPlatform'
uid=image['metadata']['uid'];deadline=time.monotonic()+90
while time.monotonic()<deadline:
    readers=json.loads(call(['-n',o['applicationNamespace'],'get','pods','-l','kimage.dev/image-uid='+uid,'-o','json']))['items']
    if not readers:break
    time.sleep(1)
else:raise RuntimeError('failed platform readers not cleaned')
releases=json.loads(call(['-n',o['applicationNamespace'],'get','imagereleases','-o','json']))['items']
assert not any(r['spec']['imageUid']==uid and r.get('status',{}).get('verified') for r in releases)
a.results.write_text(json.dumps(dict(platform=dict(noFalseReadiness=True,noVerifiedRelease=True,permanentFailure=True,readerCleaned=True,reason='WrongPlatform'),operatorRoot=inventory['components']['operator']['root']['digest'],developmentReplacementOnly=True),indent=2)+'\n')
print('actual wrong-platform failure is permanent and all owned readers were cleaned',flush=True)

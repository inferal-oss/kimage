#!/usr/bin/env python3
"""Real negative archive imports on a loopback-only isolated Kubernetes fixture."""
import argparse,base64,hashlib,io,json,os,pathlib,re,subprocess,tarfile,time
p=argparse.ArgumentParser(description=__doc__)
for name in ['kubeconfig','options','archive','probe','out']:p.add_argument('--'+name,type=pathlib.Path,required=True)
p.add_argument('--cases', nargs='+', choices=['truncated','size','missing','digest','platform'], default=['truncated','size','missing','digest','platform'])
a=p.parse_args();assert re.search(r'server: https://127\.0\.0\.1:[0-9]+\s',a.kubeconfig.read_text())
o=json.loads(a.options.read_text());assert o['runtimeProfile']=='k3s';app=o['applicationNamespace'];a.out.mkdir(parents=True,exist_ok=True)
assert (app,o['namespace'],o['installation'])==('kimage-app','kimage-system','kimage-clean')
nonce=os.urandom(4).hex();labels={'kimage.dev/installation':o['installation'],'kimage.dev/negative-case':nonce}
k=['kubectl','--kubeconfig',str(a.kubeconfig),'--request-timeout=30s','-n',app]
def call(args,body=None):
    for attempt in range(3 if args[0]=='get' else 1):
        result=subprocess.run(k+args,input=None if body is None else json.dumps(body).encode(),capture_output=True,timeout=90)
        if result.returncode==0:return result.stdout.decode()
        if args[0]!='get' or attempt==2:raise RuntimeError(result.stderr.decode()[:1500])
        time.sleep(0.5*(attempt+1))
def get(kind,name):return json.loads(call(['get',kind,name,'-o','json']))
def wait(predicate,what,limit=180):
    end=time.monotonic()+limit
    while time.monotonic()<end:
        v=predicate()
        if v:return v
        time.sleep(1)
    raise RuntimeError(what+' timeout')
helper=a.probe.read_bytes();assert len(helper)<=512*1024
name=o['installation']+'-negative-helper-'+nonce
call(['create','-f','-'],dict(apiVersion='v1',kind='ConfigMap',metadata=dict(name=name,labels=labels),immutable=True,binaryData=dict(helper=base64.b64encode(helper).decode())))
archive=(a.archive/'probe.oci.tar').read_bytes();root=json.loads((a.archive/'probe.root.json').read_text())
variants={'truncated':archive[:len(archive)//2],'size':archive}
descriptors={}
with tarfile.open(fileobj=io.BytesIO(archive)) as source:
    target=io.BytesIO();removed=False
    with tarfile.open(fileobj=target,mode='w') as output:
        for member in source:
            if member.name.startswith('blobs/sha256/') and not removed:removed=True;continue
            output.addfile(member,source.extractfile(member) if member.isfile() else None)
    variants['missing']=target.getvalue()
with tarfile.open(fileobj=io.BytesIO(archive)) as source:
    members={member.name:source.extractfile(member).read() for member in source if member.isfile()}
    first=next(member for member in source.getmembers() if member.name.startswith('blobs/sha256/'))
    damaged=bytearray(archive);damaged[first.offset_data]^=1;variants['digest']=bytes(damaged)
def blob(data,media):return dict(mediaType=media,digest='sha256:'+hashlib.sha256(data).hexdigest(),size=len(data))
manifest=json.loads(members['blobs/sha256/'+root['digest'][7:]])
config=json.loads(members['blobs/sha256/'+manifest['config']['digest'][7:]]);config['architecture']='arm64'
config_bytes=json.dumps(config,separators=(',',':')).encode();manifest['config']=blob(config_bytes,manifest['config']['mediaType'])
manifest_bytes=json.dumps(manifest,separators=(',',':')).encode();platform_root=blob(manifest_bytes,root['mediaType'])
members['blobs/sha256/'+manifest['config']['digest'][7:]]=config_bytes
members['blobs/sha256/'+platform_root['digest'][7:]]=manifest_bytes
members['index.json']=json.dumps(dict(schemaVersion=2,manifests=[platform_root])).encode()
target=io.BytesIO()
with tarfile.open(fileobj=target,mode='w') as out:
    for key,data in members.items():
        header=tarfile.TarInfo(key);header.size=len(data);header.mode=0o444;out.addfile(header,io.BytesIO(data))
variants['platform']=target.getvalue();descriptors['platform']=platform_root
results={}
for case,data in variants.items():
    if case not in a.cases:continue
    resource=o['installation']+'-bad-'+case+'-'+nonce
    path=a.out/(case+'.tar');path.write_bytes(data)
    call(['create','-f','-'],dict(apiVersion='v1',kind='PersistentVolumeClaim',metadata=dict(name=resource,labels=labels),spec=dict(accessModes=['ReadWriteOnce'],storageClassName=o['storageClass'],resources=dict(requests=dict(storage='1Gi')))))
    pod=dict(apiVersion='v1',kind='Pod',metadata=dict(name=resource),spec=dict(automountServiceAccountToken=False,restartPolicy='Never',affinity=dict(nodeAffinity=dict(requiredDuringSchedulingIgnoredDuringExecution=dict(nodeSelectorTerms=[dict(matchFields=[dict(key='metadata.name',operator='In',values=[o['node']])])]))),containers=[dict(name='producer',image=o['carrier'],imagePullPolicy='Never',command=['/helper/helper'],securityContext=dict(runAsUser=0,runAsGroup=0,allowPrivilegeEscalation=False,readOnlyRootFilesystem=True,capabilities=dict(drop=['ALL'])),resources=dict(requests=dict(cpu='10m',memory='16Mi'),limits=dict(cpu='500m',memory='64Mi')),volumeMounts=[dict(name='helper',mountPath='/helper',readOnly=True),dict(name='source',mountPath='/source')])],volumes=[dict(name='helper',configMap=dict(name=name,defaultMode=365)),dict(name='source',persistentVolumeClaim=dict(claimName=resource))]))
    call(['create','-f','-'],pod)
    wait(lambda:any(c['type']=='Ready' and c['status']=='True' for c in get('pod',resource).get('status',{}).get('conditions',[])),'producer')
    with path.open('rb') as stdin:subprocess.run(k+['exec','-i',resource,'--','/helper/helper','--stage-fixture'],stdin=stdin,check=True)
    call(['--request-timeout=0','delete','pod',resource,'--wait=true','--timeout=60s','--grace-period=1'])
    descriptor=dict(descriptors.get(case,root))
    if case=='size':descriptor['size']+=1
    call(['create','-f','-'],dict(apiVersion='kimage.dev/v1alpha1',kind='Image',metadata=dict(name=resource),spec=dict(version='1.0.0',platform=dict(os='linux',architecture='amd64'),source=dict(ociArchive=dict(claimName=resource,path='input.oci.tar',root=descriptor,producerHandoff=True)))))
    def failed():
        image=get('image',resource)
        cs=image.get('status',{}).get('conditions',[])
        assert not any(c['type']=='Ready' and c['status']=='True' for c in cs),'false readiness'
        return image if any(c['type']=='Ready' and c['status']=='False' and c['reason'] not in ['Capturing','Verified','Publishing','Sealing'] for c in cs) else None
    image=wait(failed,'negative import')
    releases=json.loads(call(['get','imagereleases','-o','json']))['items']
    assert not any(r['spec']['imageUid']==image['metadata']['uid'] and r.get('status',{}).get('verified') for r in releases)
    wait(lambda:get('image',resource).get('status',{}).get('operation',{}).get('phase')=='Failed','permanent validation failure')
    wait(lambda:not json.loads(call(['get','pods','-l','kimage.dev/image-uid='+image['metadata']['uid'],'-o','json']))['items'],'reader cleanup')
    results[case]=dict(noFalseReadiness=True,noVerifiedRelease=True,permanentFailure=True,readerCleaned=True,reason=image['status']['conditions'][0]['reason'])
    (a.out/'results.json').write_text(json.dumps(results,indent=2)+'\n')
    print(case+' import refused without a verified release',flush=True)
(a.out/'results.json').write_text(json.dumps(results,indent=2)+'\n')

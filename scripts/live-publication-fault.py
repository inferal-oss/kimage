#!/usr/bin/env python3
"""Actual API publication failure, generation replacement and retained recovery.

The temporary webhook denies only ImageRelease/status updates in the private
fixture application namespace. It never touches registry bytes or shared clusters.
"""
import argparse, base64, importlib.util, json, os, pathlib, re, subprocess, time
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--kubeconfig',type=pathlib.Path,required=True)
p.add_argument('--options',type=pathlib.Path,required=True)
p.add_argument('--installer',type=pathlib.Path,required=True)
p.add_argument('--bundle',type=pathlib.Path,required=True)
p.add_argument('--trusted-inventory',required=True)
p.add_argument('--admission-credentials',type=pathlib.Path,required=True)
p.add_argument('--probe',type=pathlib.Path,required=True)
p.add_argument('--output',type=pathlib.Path,required=True)
a=p.parse_args()
assert re.search(r'server: https://127\.0\.0\.1:[0-9]+\s',a.kubeconfig.read_text()),'private loopback fixture required'
o=json.loads(a.options.read_text()); ns=o['applicationNamespace']; system=o['namespace']; prefix=o['installation']
assert (ns,system,prefix,o['runtimeProfile'])==('kimage-app','kimage-system','kimage-clean','k3s')
b=['kubectl','--kubeconfig',str(a.kubeconfig),'--request-timeout=20s']
env=os.environ.copy();env['KUBECONFIG']=str(a.kubeconfig.resolve())
for key in ['KIMAGE_API_SERVER','KIMAGE_TLS_SERVER_NAME']:env.pop(key,None)
def run(args,body=None,success=True,timeout=90):
    r=subprocess.run(b+args,input=None if body is None else json.dumps(body).encode(),capture_output=True,env=env,timeout=timeout)
    if success and r.returncode:raise RuntimeError(r.stderr.decode()[:1500])
    return r
def get(kind,name,namespace=ns):
    return json.loads(run(([] if namespace is None else ['-n',namespace])+['get',kind,name,'-o','json']).stdout)
def create(v,namespace=ns):return json.loads(run(([] if namespace is None else ['-n',namespace])+['create','-f','-','-o','json'],v).stdout)
def wait(fn,label,seconds=300):
    deadline=time.monotonic()+seconds
    while time.monotonic()<deadline:
        result=fn()
        if result:return result
        time.sleep(1)
    raise RuntimeError(label+' timeout')
def gone(kind,name,namespace=ns):
    result=run(([] if namespace is None else ['-n',namespace])+['get',kind,name],success=False)
    if not result.returncode:return False
    if b'(NotFound)' in result.stderr:return True
    raise RuntimeError(result.stderr.decode()[:1500])
def remove(kind,name,namespace=ns):
    v=get(kind,name,namespace); plural={'pod':'pods','pvc':'persistentvolumeclaims','configmap':'configmaps','validatingwebhookconfiguration':'validatingwebhookconfigurations'}[kind]
    path='/apis/admissionregistration.k8s.io/v1/'+plural+'/'+name if namespace is None else '/api/v1/namespaces/'+namespace+'/'+plural+'/'+name
    body=dict(apiVersion='v1',kind='DeleteOptions',preconditions=dict(uid=v['metadata']['uid'],resourceVersion=v['metadata']['resourceVersion']))
    if kind=='pod':body['gracePeriodSeconds']=1
    run(['delete','--raw',path,'-f','-'],body)
    wait(lambda:gone(kind,name,namespace),'delete '+name,90)
def root_triple(root):return {k:root[k] for k in ['mediaType','digest','size']}
a.output.mkdir(parents=True,exist_ok=True)
module=importlib.util.spec_from_file_location('kimage_fixture_pack',pathlib.Path(__file__).with_name('package-bundle.py'))
pack=importlib.util.module_from_spec(module);module.loader.exec_module(pack)
nonce=os.urandom(8).hex(); name=prefix+'-fault-'+nonce[:8]
roots=[]
for index in range(2):
    root=pack.image(a.probe,a.output/('input-'+str(index)+'.tar'),'kimage-probe',nonce+'-'+str(index))
    (a.output/('root-'+str(index)+'.json')).write_text(json.dumps(root));roots.append(root)
assert roots[0]['digest']!=roots[1]['digest']
probe=a.probe.read_bytes();assert len(probe)<=512*1024
labels={'kimage.dev/installation':prefix,'kimage.dev/fault-case':nonce}
cm=create(dict(apiVersion='v1',kind='ConfigMap',metadata=dict(name=name,labels=labels),immutable=True,binaryData=dict(probe=base64.b64encode(probe).decode())))
def stage(index):
    source=name+'-'+str(index);producer=source+'-producer'
    create(dict(apiVersion='v1',kind='PersistentVolumeClaim',metadata=dict(name=source,labels=labels),spec=dict(accessModes=['ReadWriteOnce'],storageClassName=o['storageClass'],resources=dict(requests=dict(storage='1Gi')))))
    create(dict(apiVersion='v1',kind='Pod',metadata=dict(name=producer,labels=labels),spec=dict(automountServiceAccountToken=False,restartPolicy='Never',affinity=dict(nodeAffinity=dict(requiredDuringSchedulingIgnoredDuringExecution=dict(nodeSelectorTerms=[dict(matchFields=[dict(key='metadata.name',operator='In',values=[o['node']])])]))),containers=[dict(name='producer',image=o['carrier'],imagePullPolicy='Never',command=['/fixture/probe'],securityContext=dict(runAsUser=0,runAsGroup=0,allowPrivilegeEscalation=False,readOnlyRootFilesystem=True,capabilities=dict(drop=['ALL'])),resources=dict(requests=dict(cpu='10m',memory='16Mi'),limits=dict(cpu='500m',memory='64Mi')),volumeMounts=[dict(name='source',mountPath='/source'),dict(name='fixture',mountPath='/fixture',readOnly=True)])],volumes=[dict(name='source',persistentVolumeClaim=dict(claimName=source)),dict(name='fixture',configMap=dict(name=name,defaultMode=365))])))
    wait(lambda:any(c['type']=='Ready' and c['status']=='True' for c in get('pod',producer).get('status',{}).get('conditions',[])),'producer readiness')
    with (a.output/('input-'+str(index)+'.tar')).open('rb') as stream:
        subprocess.run(b+['--request-timeout=0','-n',ns,'exec','-i',producer,'--','/fixture/probe','--stage-fixture'],stdin=stream,stdout=subprocess.DEVNULL,env=env,check=True,timeout=60)
    remove('pod',producer)
    return source
sources=[stage(0),stage(1)]
# API-backed negative usage/pin checks: both preserve UID/RV and deletion state.
retained='release-a540494123bdf27af0f954deb083f83576676009'
before=get('imagerelease',retained)
for command in [['delete','imagerelease',retained,'--dry-run=server'],['patch','imagerelease',retained,'--type=merge','--dry-run=server','-p',json.dumps(dict(metadata=dict(finalizers=[])))]]:
    result=run(['-n',ns]+command,success=False)
    assert result.returncode and b'RetainedReleasePinned' in result.stderr,'published release pin was not enforced'
after=get('imagerelease',retained)
assert before['metadata']['uid']==after['metadata']['uid'] and before['metadata']['resourceVersion']==after['metadata']['resourceVersion'] and not after['metadata'].get('deletionTimestamp')
webhook_name=name+'-seal'
base_hook=get('validatingwebhookconfiguration',prefix+'-retention',None)
hook=base_hook['webhooks'][0]
hook['name']=webhook_name+'.fixture.kimage.dev';hook['clientConfig']['service']['path']='/mutate'
hook['rules']=[dict(apiGroups=['kimage.dev'],apiVersions=['v1alpha1'],operations=['UPDATE'],resources=['imagereleases/status'],scope='Namespaced')]
create(dict(apiVersion='admissionregistration.k8s.io/v1',kind='ValidatingWebhookConfiguration',metadata=dict(name=webhook_name,labels=labels),webhooks=[hook]),None)
fault_active=True
try:
    def fault_ready():
        result=run(['-n',ns,'patch','imagerelease',retained,'--subresource=status','--type=merge','--dry-run=server','-p',json.dumps(dict(status=dict(verified=True)))],success=False)
        return result.returncode and webhook_name.encode() in result.stderr and b'denied the request' in result.stderr
    wait(fault_ready,'temporary API denial activation',60)
    spec=dict(version='1.0.0',platform=dict(os='linux',architecture='amd64'),source=dict(ociArchive=dict(claimName=sources[0],path='input.oci.tar',root=roots[0],producerHandoff=True)))
    image=create(dict(apiVersion='kimage.dev/v1alpha1',kind='Image',metadata=dict(name=name,labels=labels),spec=spec));uid=image['metadata']['uid']
    def pending(root,generation):
        values=json.loads(run(['-n',ns,'get','imagereleases','-o','json']).stdout)['items']
        matches=[v for v in values if v['spec']['imageUid']==uid and v['spec']['sourceGeneration']==generation and root_triple(v['spec']['root'])==root_triple(root)]
        if not matches:return None
        assert len(matches)==1 and not matches[0].get('status',{}).get('verified',False)
        assert not any(c['type']=='Ready' and c['status']=='True' for c in get('image',name).get('status',{}).get('conditions',[]))
        return matches[0]
    first=wait(lambda:pending(roots[0],1),'first unacknowledged release')
    start_attempt=get('image',name).get('status',{}).get('operation',{}).get('attempt',1)
    time.sleep(10)
    state=get('image',name)
    assert state['status']['operation']['attempt']-start_attempt<=4,'status feedback bypassed exponential delay'
    assert any(c['type']=='RetryAfter' for c in state['status']['conditions']),'durable retry deadline missing'
    spec['source']['ociArchive'].update(claimName=sources[1],root=roots[1])
    run(['-n',ns,'patch','image',name,'--type=merge','-p',json.dumps(dict(spec=spec))])
    second=wait(lambda:pending(roots[1],2),'replacement unacknowledged release')
    assert first['metadata']['uid']!=second['metadata']['uid']
    # UID/version naming is deliberately stable when obsolete unverified
    # metadata is replaced. Its UID changes while the resource name is reused.
    assert get('imagerelease',first['metadata']['name'])['metadata']['uid']==second['metadata']['uid']
    # Remove every source and reader before resuming: recovery must use retained OCI.
    readers=json.loads(run(['-n',ns,'get','pods','-l','kimage.dev/image-uid='+uid,'-o','json']).stdout)['items']
    for reader in readers:
        assert any(x['uid']==uid and x['kind']=='Image' for x in reader['metadata'].get('ownerReferences',[]))
        remove('pod',reader['metadata']['name'])
    for source in sources:remove('pvc',source)
    def backed_off():
        value=get('image',name)
        if value.get('status',{}).get('operation',{}).get('attempt',0)<5:return None
        deadlines=[int(c['message']) for c in value['status']['conditions'] if c['type']=='RetryAfter']
        return value if deadlines and deadlines[0]-time.time()>15 else None
    delayed=wait(backed_off,'durable retry before restart',180)
    attempt=delayed['status']['operation']['attempt']
    operator=get('pod',prefix+'-operator',system)
    replacement=dict(apiVersion='v1',kind='Pod',metadata=dict(name=operator['metadata']['name'],labels=operator['metadata']['labels'],annotations=operator['metadata'].get('annotations',{})),spec=operator['spec'])
    remove('pod',prefix+'-operator',system)
    create(replacement,system)
    wait(lambda:any(c['type']=='Ready' and c['status']=='True' for c in get('pod',prefix+'-operator',system).get('status',{}).get('conditions',[])),'operator restart')
    remaining=max(int(c['message']) for c in delayed['status']['conditions'] if c['type']=='RetryAfter')-time.time()
    assert remaining>3,'restart used the full retry window; repeat this measurement'
    time.sleep(2)
    assert get('image',name)['status']['operation']['attempt']==attempt,'restart bypassed durable backoff'
    remove('validatingwebhookconfiguration',webhook_name,None);fault_active=False
    def ready():
        value=get('image',name)
        return value if value.get('status',{}).get('observedGeneration')==2 and any(c['type']=='Ready' and c['status']=='True' for c in value.get('status',{}).get('conditions',[])) else None
    wait(ready,'retained publication recovery')
    release=get('imagerelease',second['metadata']['name'])
    assert release['metadata']['uid']==second['metadata']['uid'] and release['status']['verified']
    assert root_triple(release['spec']['root'])==root_triple(roots[1])
    # Both old orphaned bytes and the selected new root remain independently readable.
    for index in range(2):
        subprocess.run([str(a.installer.resolve()),'verify-remote','--root',str(a.output/('root-'+str(index)+'.json')),'--namespace',system,'--pod',prefix+'-registry','--origin',f'https://{prefix}-registry.{system}.svc:5443/','--repository',ns+'/'+uid,'--ca',str(pathlib.Path(o['credentials'])/'ca.crt'),'--password-file',str(pathlib.Path(o['credentials'])/'publisher-password')],env=env,check=True,timeout=90)
    proof=name+'-runtime'
    create(dict(apiVersion='v1',kind='Pod',metadata=dict(name=proof,labels=labels),spec=dict(automountServiceAccountToken=False,containers=[dict(name='app',image=f'platform://{ns}/{name}:1.0.0',resources=dict(requests=dict(cpu='10m',memory='16Mi'),limits=dict(cpu='100m',memory='64Mi')))])))
    wait(lambda:any(c['type']=='Ready' and c['status']=='True' for c in get('pod',proof).get('status',{}).get('conditions',[])),'recovered runtime Pod')
    pod=get('pod',proof);assert pod['spec']['containers'][0]['image'].endswith('@'+roots[1]['digest']) and pod['spec']['containers'][0]['imagePullPolicy']=='IfNotPresent'
    metrics=run(['get','--raw',f'/api/v1/namespaces/{system}/pods/{prefix}-operator:8080/proxy/metrics']).stdout
    assert b'kimage_operations_active' in metrics and len(metrics)<4096 and uid.encode() not in metrics
    evidence=dict(imageUid=uid,releaseUid=release['metadata']['uid'],generation=2,publishedPinDeleteDenied=True,publishedUnpinDenied=True,noFalseReadinessDuringApiFailure=True,obsoleteUnverifiedMetadataRemoved=True,oldRegistryGraphRetained=True,sourcePvcAbsentOnRecovery=True,readerAbsentOnRecovery=True,operatorRestarted=True,durableBackoffAcrossWatchAndRestart=True,recoveredReleaseUidUnchanged=True,runtimePodReady=True,boundedMetrics=True,newRoot=roots[1],oldRoot=roots[0])
    (a.output/'evidence.json').write_text(json.dumps(evidence,indent=2)+'\n')
    remove('pod',proof);remove('configmap',name)
    print('real API publication denial, stale generation cleanup, retained recovery and ordinary Pod passed',flush=True)
finally:
    if fault_active:remove('validatingwebhookconfiguration',webhook_name,None)

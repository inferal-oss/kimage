#!/usr/bin/env python3
"""Owned isolated-fixture policy/RBAC/schema preflight refusal checks."""
import argparse,json,os,pathlib,re,subprocess,time
p=argparse.ArgumentParser(description=__doc__)
for n in ['kubeconfig','options','installer','bundle','admission-credentials','output']:p.add_argument('--'+n,type=pathlib.Path,required=True)
p.add_argument('--trusted-inventory',required=True)
a=p.parse_args();assert re.search(r'server: https://127\.0\.0\.1:[0-9]+\s',a.kubeconfig.read_text())
o=json.loads(a.options.read_text());assert (o['namespace'],o['installation'],o['runtimeProfile'])==('kimage-system','kimage-clean','k3s')
a.output.mkdir(parents=True,exist_ok=True);os.chmod(a.output,0o700)
nonce=os.urandom(4).hex();name='kimage-denied-'+nonce
labels={'kimage.dev/installation':name}
b=['kubectl','--kubeconfig',str(a.kubeconfig),'--request-timeout=20s']
e=os.environ.copy();e['KUBECONFIG']=str(a.kubeconfig.resolve())
for key in ['KIMAGE_API_SERVER','KIMAGE_TLS_SERVER_NAME']:e.pop(key,None)
def call(args,body=None):return subprocess.run(b+args,input=None if body is None else json.dumps(body).encode(),capture_output=True,check=True,timeout=90).stdout
def create(body):return json.loads(call(['create','-f','-','-o','json'],body))
def get(kind,n,ns=None):return json.loads(call(([] if ns is None else ['-n',ns])+['get',kind,n,'-o','json']))
def flags(options):
    out=[]
    for k,v in options.items():out+=['--'+''.join('-'+c.lower() if c.isupper() else c for c in k),str(v)]
    return out
create(dict(apiVersion='v1',kind='Namespace',metadata=dict(name=name,labels=dict(labels,**{'pod-security.kubernetes.io/enforce':'restricted','pod-security.kubernetes.io/enforce-version':'latest'}))))
credentials=a.output/'credentials'
subprocess.run(['python3',str(pathlib.Path(__file__).with_name('prepare-test-credentials.py')),'--dir',str(credentials),'--registry-dns',name+'-registry.'+name+'.svc'],check=True)
options=dict(o);options['namespace']=name;options['installation']=name;options['credentials']=str(credentials.resolve());options.pop('applicationNamespace')
common=['--bundle',str(a.bundle.resolve()),'--trusted-inventory',a.trusted_inventory]
def no_data():
    values=json.loads(call(['-n',name,'get','pods,pvc,configmaps,secrets,services','-l','kimage.dev/installation='+name,'-o','json']))['items']
    assert not values,'bootstrap created data before refusing the prerequisite'
result=subprocess.run([str(a.installer.resolve()),'bootstrap']+flags(options)+common,env=e,capture_output=True,timeout=90)
assert result.returncode and b'BootstrapAdmissionDenied' in result.stderr,result.stderr.decode()[:600]
no_data()
# A short-lived installer identity can read prerequisites, but cannot create Pods.
sa=create(dict(apiVersion='v1',kind='ServiceAccount',metadata=dict(name=name,namespace=name,labels=labels),automountServiceAccountToken=False))
role=create(dict(apiVersion='rbac.authorization.k8s.io/v1',kind='ClusterRole',metadata=dict(name=name,labels=labels),rules=[dict(apiGroups=[''],resources=['nodes'],resourceNames=[o['node']],verbs=['get']),dict(apiGroups=[''],resources=['namespaces'],resourceNames=[name],verbs=['get']),dict(apiGroups=['authorization.k8s.io'],resources=['selfsubjectaccessreviews'],verbs=['create'])]))
binding=create(dict(apiVersion='rbac.authorization.k8s.io/v1',kind='ClusterRoleBinding',metadata=dict(name=name,labels=labels),subjects=[dict(kind='ServiceAccount',name=name,namespace=name)],roleRef=dict(apiGroup='rbac.authorization.k8s.io',kind='ClusterRole',name=name)))
try:
    token=call(['-n',name,'create','token',name,'--duration=10m']).decode().strip()
    cluster=json.loads(call(['config','view','--raw','--minify','-o','jsonpath={.clusters[0].cluster}']))
    limited=dict(apiVersion='v1',kind='Config',clusters=[dict(name='fixture',cluster=cluster)],users=[dict(name='limited',user=dict(token=token))],contexts=[dict(name='limited',context=dict(cluster='fixture',user='limited',namespace=name))],**{'current-context':'limited'})
    limited_file=a.output/'limited-config.json';limited_file.write_text(json.dumps(limited));limited_file.chmod(0o600)
    limited_env=dict(e,KUBECONFIG=str(limited_file.resolve()))
    result=subprocess.run([str(a.installer.resolve()),'bootstrap']+flags(options)+common,env=limited_env,capture_output=True,timeout=90)
    assert result.returncode and b'BootstrapPermissionDenied' in result.stderr,result.stderr.decode()[:600]
    no_data()
finally:
    for kind,value in [('clusterrolebindings',binding),('clusterroles',role)]:
        body=dict(apiVersion='v1',kind='DeleteOptions',preconditions=dict(uid=value['metadata']['uid'],resourceVersion=value['metadata']['resourceVersion']))
        call(['delete','--raw','/apis/rbac.authorization.k8s.io/v1/'+kind+'/'+name,'-f','-'],body)
    body=dict(apiVersion='v1',kind='DeleteOptions',preconditions=dict(uid=sa['metadata']['uid'],resourceVersion=sa['metadata']['resourceVersion']))
    call(['delete','--raw','/api/v1/namespaces/'+name+'/serviceaccounts/'+name,'-f','-'],body)
# Only a metadata revision marker changes; the actual schema, data and UID stay.
crd=get('crd','images.kimage.dev');original=crd['metadata']['annotations']['kimage.dev/schema-revision']
assert original=='2'
state=get('configmap','kimage-clean-control-state','kimage-system')
call(['patch','crd','images.kimage.dev','--type=merge','-p',json.dumps(dict(metadata=dict(uid=crd['metadata']['uid'],resourceVersion=crd['metadata']['resourceVersion'],annotations={'kimage.dev/schema-revision':'999'})))])
try:
    result=subprocess.run([str(a.installer.resolve()),'control-plane']+flags(o)+common+['--admission-credentials',str(a.admission_credentials.resolve())],env=e,capture_output=True,timeout=90)
    assert result.returncode and b'UnsupportedSchemaRevision' in result.stderr,result.stderr.decode()[:600]
    current=get('crd','images.kimage.dev');assert current['metadata']['uid']==crd['metadata']['uid'] and current['spec']==crd['spec'] and current['metadata']['annotations']['kimage.dev/schema-revision']=='999'
    assert get('configmap','kimage-clean-control-state','kimage-system')['metadata']['resourceVersion']==state['metadata']['resourceVersion']
finally:
    current=get('crd','images.kimage.dev');assert current['metadata']['uid']==crd['metadata']['uid'] and current['metadata']['annotations']['kimage.dev/schema-revision']=='999'
    call(['patch','crd','images.kimage.dev','--type=merge','-p',json.dumps(dict(metadata=dict(uid=current['metadata']['uid'],resourceVersion=current['metadata']['resourceVersion'],annotations={'kimage.dev/schema-revision':original})))])
(a.output/'results.json').write_text(json.dumps(dict(namespaceAdmissionDeniedBeforeDataWrites=True,installerRbacDeniedBeforeDataWrites=True,incompatibleSchemaRefusedBeforeControlWrites=True,schemaSpecAndUidPreserved=True,temporaryRevisionRestored=True,shortLivedInstallerIdentityRemoved=True,namespaceRetained=name),indent=2)+'\n')
print('real policy, RBAC and schema incompatibility refusals passed without bootstrap data writes',flush=True)

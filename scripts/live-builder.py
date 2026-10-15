#!/usr/bin/env python3
"""Exercise the approved offline worker in a private loopback-API fixture."""
import argparse, base64, hashlib, json, os, pathlib, subprocess, tarfile, time
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--kubeconfig',type=pathlib.Path,required=True)
p.add_argument('--bundle',type=pathlib.Path,required=True)
p.add_argument('--probe',type=pathlib.Path,required=True)
p.add_argument('--output',type=pathlib.Path,required=True)
p.add_argument('--installer',type=pathlib.Path,required=True)
p.add_argument('--run-sandbox',action='store_true')
a=p.parse_args()
assert 'server: https://127.0.0.1:27643' in a.kubeconfig.read_text(), 'owned isolated fixture required'
ns='kimage-system'; prefix='kimage-clean'; dns=prefix+'-builder.'+ns+'.svc'
b=['kubectl','--kubeconfig',str(a.kubeconfig),'--request-timeout=45s','-n',ns]
def cmd(args, body=None):
    r=subprocess.run(b+args,input=None if body is None else json.dumps(body).encode(),capture_output=True,timeout=90)
    if r.returncode:raise RuntimeError(r.stderr.decode()[:2000])
    return r.stdout
def create(v):
    cmd(['create','-f','-'],v)
labels={'kimage.dev/installation':prefix,'kimage.dev/role':'builder-client'}
probe=a.probe.read_bytes();assert len(probe)<=512*1024
nonce=os.urandom(16).hex(); name=prefix+'-builder-proof-'+nonce[:8]
inventory=json.loads((a.bundle/'inventory.json').read_text())
root=inventory['components']['builder']['root']
image='127.0.0.1:17641/'+ns+'/system-builder@'+root['digest']
definition=('FROM scratch\nCOPY --chmod=0555 archive /probe\nRUN --network=none ["/probe", "--dns-fixture", "127.0.0.1:1234"]\nLABEL fixture="'+nonce+'"\n') if a.run_sandbox else 'FROM scratch\nCOPY evidence /evidence\n'
create(dict(apiVersion='v1',kind='ConfigMap',metadata=dict(name=name,labels=labels),immutable=True,
    data={'Containerfile':definition,'evidence':nonce},binaryData={'probe':base64.b64encode(probe).decode()}))
pod=dict(apiVersion='v1',kind='Pod',metadata=dict(name=name,labels=labels),spec=dict(
    automountServiceAccountToken=False,nodeName=prefix,restartPolicy='Never',
    securityContext=dict(seccompProfile=dict(type='RuntimeDefault')),
    containers=[dict(name='client',image=image,imagePullPolicy='IfNotPresent',command=['/fixture/probe'],
        env=[dict(name='OTEL_TRACES_EXPORTER',value='none')],
        resources=dict(requests=dict(cpu='10m',memory='16Mi'),limits=dict(cpu='1',memory='256Mi',**{'ephemeral-storage':'256Mi'})),
        securityContext=dict(runAsUser=65532,runAsGroup=65532,runAsNonRoot=True,allowPrivilegeEscalation=False,readOnlyRootFilesystem=True,capabilities=dict(drop=['ALL'])),
        volumeMounts=[dict(name='input',mountPath='/fixture',readOnly=True),dict(name='credentials',mountPath='/credentials',readOnly=True)])],
    volumes=[dict(name='input',configMap=dict(name=name,defaultMode=365)),dict(name='credentials',secret=dict(secretName=prefix+'-builder-client',defaultMode=292))]))
if a.run_sandbox:
    input_image='127.0.0.1:17641/'+ns+'/system-input@'+inventory['components']['input']['root']['digest']
    secure=dict(runAsUser=65532,runAsGroup=65532,allowPrivilegeEscalation=False,readOnlyRootFilesystem=True,capabilities=dict(drop=['ALL']))
    resources=dict(requests=dict(cpu='10m',memory='16Mi'),limits=dict(cpu='500m',memory='128Mi'))
    pod['spec']['volumes'].append(dict(name='snapshot',emptyDir=dict(sizeLimit='16Mi')))
    pod['spec']['initContainers']=[dict(name='prepare',image=input_image,imagePullPolicy='IfNotPresent',command=['/kimage-input','prepare'],securityContext=dict(secure,runAsUser=0,runAsGroup=0),resources=resources,volumeMounts=[dict(name='snapshot',mountPath='/snapshot')])]
    pod['spec']['containers'].append(dict(name='reader',image=input_image,imagePullPolicy='IfNotPresent',command=['/kimage-input','capture','--path','probe'],securityContext=secure,resources=resources,volumeMounts=[dict(name='input',mountPath='/source/probe',subPath='probe',readOnly=True),dict(name='snapshot',mountPath='/snapshot')],readinessProbe=dict(exec=dict(command=['/kimage-input','ready']),periodSeconds=1)))
    pod['spec']['containers'][0]['volumeMounts'].append(dict(name='snapshot',mountPath='/context',readOnly=True))
create(pod)
deadline=time.monotonic()+180
while time.monotonic()<deadline:
    v=json.loads(cmd(['get','pod',name,'-o','json']))
    if any(c['type']=='Ready' and c['status']=='True' for c in v.get('status',{}).get('conditions',[])):break
    time.sleep(1)
else:raise RuntimeError('builder client readiness timeout')
args=['/buildctl','--addr','tcp://'+dns+':1234','--tlsservername',dns,'--tlscacert','/credentials/ca.crt','--tlscert','/credentials/client.crt','--tlskey','/credentials/client.key']
a.output.mkdir(parents=True,exist_ok=True)
archive=a.output/'result.oci.tar'
with archive.open('wb') as out:
    r=subprocess.run(b+['--request-timeout=0','exec',name,'-c','client','--']+args+['build','--frontend','dockerfile.v0','--local','context='+('/context' if a.run_sandbox else '/fixture'),'--local','dockerfile=/fixture','--opt','filename=Containerfile','--opt','platform=linux/amd64','--opt','force-network-mode=none','--output','type=oci','--progress','plain'],stdout=out,stderr=subprocess.PIPE,timeout=180)
assert r.returncode==0, 'worker solve failed: '+r.stderr.decode()[-4000:]
assert archive.stat().st_size<=2*1024*1024
with tarfile.open(archive) as tar:
    entry=tar.getmember('index.json'); assert entry.size<65536
    index=json.load(tar.extractfile(entry));assert len(index['manifests'])==1
    result=index['manifests'][0]
rootfile=a.output/'result.root.json';rootfile.write_text(json.dumps(result))
subprocess.run([str(a.installer.resolve()),'verify','--archive',str(archive),'--root',str(rootfile)],check=True)
worker=json.loads(cmd(['get','pod',prefix+'-builder','-o','json']))
assert worker['spec']['hostUsers'] is False and worker['spec'].get('hostNetwork',False) is False
assert all(not c.get('securityContext',{}).get('privileged',False) for c in worker['spec']['containers'])
assert all('hostPath' not in v for v in worker['spec']['volumes'])
(a.output/'evidence.json').write_text(json.dumps(dict(workerUid=worker['metadata']['uid'],hostUsers=False,privileged=False,runtimeSocket=False,processSandbox=True,runInstruction=a.run_sandbox,buildkitVersion='0.33.0',frontend='dockerfile.v0',frontendVersion='1.27.0',root=result,uniqueInput=hashlib.sha256(nonce.encode()).hexdigest(),ociVerified=True),indent=2)+'\n')
cmd(['--request-timeout=0','delete','pod',name,'--wait=true','--timeout=60s','--grace-period=1'])
cmd(['delete','configmap',name])
print('authenticated offline scratch build exported and independently verified',flush=True)

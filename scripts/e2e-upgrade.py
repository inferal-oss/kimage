#!/usr/bin/env python3
"""Explicitly enabled acceptance of published 0.4.1 -> 0.5.0 -> 0.4.1.

Uses fresh dedicated namespaces. Only fixture Node labels may be changed; Nodes
are never deleted or cordoned. Images, releases, inputs and registry PVC remain.
"""
import argparse
import base64
import copy
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import tempfile
import time
import uuid

ROOT=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location('mixed',ROOT/'scripts/e2e-mixed-cluster.py')
mixed=importlib.util.module_from_spec(spec);spec.loader.exec_module(mixed)
spec=importlib.util.spec_from_file_location('log_assertions',ROOT/'scripts/e2e-log-assertions.py')
log_assertions=importlib.util.module_from_spec(spec);spec.loader.exec_module(log_assertions)


def require(value,message):
    if not value: raise RuntimeError(message)


class Upgrade:
    def __init__(self,config):
        required={'kubeconfig','context','namespace','systemNamespace','installation','probes','output','installer','previous','catalog','chart','mode','scope','runtimeProfile','port'}
        require(required<=config.keys() and not config.keys()-required-{'nodeChanges','registryNode','kubectlCommand','timeoutSeconds'},'Invalid lifecycle fixture configuration')
        require(config['mode'] in ('cli','helm') and config['scope'] in ('namespace','cluster'),'Invalid installer manager or scope')
        require(config['runtimeProfile'] in ('inferal','k3s'),'Unsupported fixture runtime')
        self.config=config
        base={k:config[k] for k in ['kubeconfig','context','namespace','systemNamespace','installation','probes','output']}
        base['timeoutSeconds']=config.get('timeoutSeconds',600)
        self.api=mixed.MixedCluster(base)
        prefix=config.get('kubectlCommand',['kubectl'])
        require(prefix in (['kubectl'],['k3s','kubectl']),'Unsupported kubectl executable')
        self.api.command=prefix+self.api.command[1:]
        self.app,self.system,self.id=(config[k] for k in ['namespace','systemNamespace','installation'])
        self.output=self.api.output
        self.previous=Path(config['previous']).resolve()
        self.installer=Path(config['installer']).resolve()
        self.previous_binary=next(self.previous.glob('kimage-v0.4.1-*'))
        self.target=json.loads(Path(config['catalog']).read_bytes())
        require(self.target['release']=='0.5.0','Test requires an exact 0.5.0 catalog')
        verified=json.loads((self.previous/'verified.json').read_bytes())
        require(verified['release']=='0.4.1','Published predecessor verification is required')
        for asset in verified['assets']:
            data=(self.previous/asset['name']).read_bytes()
            require(len(data)==asset['size'] and 'sha256:'+hashlib.sha256(data).hexdigest()==asset['sha256'],'Predecessor asset changed')
        self.report=dict(schema=1,test='installation-lifecycle',source='0.4.1',target='0.5.0',manager=config['mode'],scope=config['scope'],checks=[],complete=False)
        self.report['harnessSha256']='sha256:'+hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
        self.report['artifacts']={key:'sha256:'+hashlib.sha256(Path(config[key]).read_bytes()).hexdigest() for key in ['installer','catalog','chart']}
        self.report['predecessor']=verified
        self.nodes={}
        self.saved_labels={}
        self.consumers=[]
        self.images=[]
        self.private_logs=[]
        self.environment=dict(os.environ)
        self.private_config=None

    def run_command(self,args,timeout=1200,ok=True):
        result=subprocess.run([str(a) for a in args],env=self.environment,text=True,capture_output=True,timeout=timeout)
        if ok and result.returncode: raise RuntimeError('Fixture command failed: '+str(args[:3])+'\n'+result.stderr[-2500:])
        return result

    def check(self,text):
        self.report['checks'].append(text)
        (self.output/'results.json').write_text(json.dumps(self.report,indent=2)+'\n')
        print('PASS: '+text,flush=True)

    def replace(self,kind,name,body,namespace=None):
        # No large OpenAPI discovery. Only explicitly supported fixture resources.
        paths={'ConfigMap':'/api/v1/namespaces/'+str(namespace)+'/configmaps/','Node':'/api/v1/nodes/'}
        return self.api.call(['replace','--raw',paths[kind]+name+'?fieldValidation=Strict','-f','-'],body)

    def preflight(self):
        require(self.api.get('namespace',self.system) is None and self.api.get('namespace',self.app) is None,'Fresh dedicated namespaces required')
        self.nodes={n['metadata']['name']:n for n in self.api.get('nodes')['items']}
        self.architectures={n['status']['nodeInfo']['architecture'] for n in self.nodes.values() if n['status']['nodeInfo']['operatingSystem']=='linux' and any(c['type']=='Ready' and c['status']=='True' for c in n['status']['conditions'])}
        require(self.architectures and self.architectures<={'amd64','arm64'},'Supported native Ready Nodes required')
        require(not self.config.get('nodeChanges') or self.architectures=={'amd64','arm64'},'Node-change acceptance requires the real mixed cluster')
        self.primary=self.config.get('registryNode') or sorted(self.nodes)[0]
        require(self.primary in self.nodes,'Missing registry node')
        require(not self.config.get('nodeChanges') or self.nodes[self.primary]['status']['nodeInfo']['architecture']=='amd64','Mixed node-change test requires an amd64 registry node')
        result=self.api.call(['config','view','--minify','--raw','-o','json'])
        config=json.loads(result.stdout)
        require(config['current-context']==self.config['context'],'Explicit kubeconfig context mismatch')
        self.private_config=tempfile.NamedTemporaryFile(prefix='kimage-lifecycle-kubeconfig-',delete=False)
        os.chmod(self.private_config.name,0o600);self.private_config.write(result.stdout.encode());self.private_config.close()
        self.environment['KUBECONFIG']=self.private_config.name
        self.environment.pop('KIMAGE_API_SERVER',None);self.environment.pop('KIMAGE_TLS_SERVER_NAME',None)
        for ns in [self.system,self.app]:
            self.api.call(['create','--raw','/api/v1/namespaces?fieldValidation=Strict','-f','-'],dict(apiVersion='v1',kind='Namespace',metadata=dict(name=ns)))
        self.probes={arch:base64.b64encode(mixed.verify_probe(Path(self.config['probes'])/('probe-'+arch),arch)).decode() for arch in sorted(self.architectures)}
        self.api.create(dict(apiVersion='v1',kind='ConfigMap',metadata=dict(name='native-probes'),immutable=True,binaryData={'probe-'+a:b for a,b in self.probes.items()}))
        self.check('fresh isolated fixture and verified native ELF inputs')

    def common(self):
        args=['--namespace',self.system,'--installation',self.id,'--scope',self.config['scope'],'--runtime-profile',self.config['runtimeProfile'],'--registry-node',self.primary,'--port',str(self.config['port'])]
        if self.config['scope']=='namespace':args+=['--application-namespace',self.app]
        else:
            # Exclude existing ordinary namespaces so a cluster-scope test never
            # adopts unrelated applications or another retained fixture.
            excluded=sorted(n['metadata']['name'] for n in self.api.get('namespaces')['items'] if n['metadata']['name'] not in (self.app,self.system) and not n['metadata']['name'].startswith('kube-'))
            for ns in excluded:args+=['--exclude-namespace',ns]
            self.excluded=excluded
        return args

    def install_previous(self):
        actual=self.run_command([self.previous_binary,'--version']).stdout.strip()
        require(actual in ('kimage 0.4.1','kimage-install 0.4.1'),'Must execute the published predecessor binary')
        common=self.common()
        if self.config['mode']=='cli':
            self.run_command([self.previous_binary,'install',*common,'--image-catalog',self.previous/'component-images.json'])
        else:
            values=dict(installation=self.id,scope=self.config['scope'],runtimeProfile=self.config['runtimeProfile'],registryNode=self.primary,port=self.config['port'])
            if self.config['scope']=='namespace':values['applicationNamespace']=self.app
            else:values['excludedNamespaces']=self.excluded
            path=self.output/'helm-values.json';path.write_text(json.dumps(values))
            self.run_command(['helm','install',self.id,self.previous/'kimage-0.4.1.tgz','--namespace',self.system,'-f',path,'--wait','--wait-for-jobs','--timeout','10m'])
        self.assert_images('0.4.1',json.loads((self.previous/'component-images.json').read_bytes()))
        self.check('actual published 0.4.1 installed with native component pulls')

    def assert_images(self,version,catalog):
        record=json.loads(self.api.get('configmap',self.id+'-installation',self.system)['data']['installation.json'])
        require(record['release']==version,'Wrong installed release')
        system=self.api.get('pods',namespace=self.system)['items']
        own=[p for p in system if p['metadata'].get('labels',{}).get('kimage.dev/installation')==self.id and not p['metadata'].get('deletionTimestamp') and p.get('status',{}).get('phase') not in ('Succeeded','Failed')]
        observed=set()
        identities=[]
        pins={catalog['repository']+'/'+role+'@'+entry['digest'] for role,entry in catalog['components'].items()}
        for pod in own:
            arch=self.nodes[pod['spec']['nodeName']]['status']['nodeInfo']['architecture']
            require(arch in self.architectures,'Non-native fixture placement')
            for status in pod.get('status',{}).get('containerStatuses',[]):
                require(status.get('imageID'),'Runtime did not report an image identity')
                identities.append(dict(role=pod['metadata'].get('labels',{}).get('kimage.dev/role'),architecture=arch,container=status['name'],imageId=status['imageID']))
            for container in pod['spec']['containers']+pod['spec'].get('initContainers',[]):
                require(container['image'] in pins,'Unpinned component image')
            if pod['metadata'].get('labels',{}).get('kimage.dev/role') in ('builder','node-proxy'):observed.add(arch)
        require(observed==self.architectures,'Components did not run on every native architecture')
        self.report.setdefault('nativeComponents',[]).append(dict(release=version,identities=identities))

    def installation_diagnostics(self):
        before=self.operation()
        last_issues=None
        def healthy():
            nonlocal last_issues
            result=self.run_command([self.installer,'doctor','--namespace',self.system,'--installation',self.id,'--json'],ok=False)
            doctor=json.loads(result.stdout) if result.stdout.strip() else dict(healthy=False,issues=[dict(reason='DoctorCommandUnavailable')])
            if result.returncode!=0 or not doctor['healthy']:
                if doctor['issues']!=last_issues:print('WAIT: installation health '+json.dumps(doctor['issues']),flush=True);last_issues=doctor['issues']
                return False
            status=json.loads(self.run_command([self.installer,'status','--namespace',self.system,'--installation',self.id,'--json']).stdout)
            return status['healthy'] and status['release']=='0.5.0'
        self.api.wait(healthy,'read-only doctor reports healthy installation')
        require(self.operation()==before,'Diagnostics changed the operation journal')
        self.check('status and doctor report healthy placement, resource pins, retained storage and certificates without changing the journal')

    def build(self,stage,architecture=None,available=None):
        expected={architecture} if architecture else (available or self.architectures)
        name='lifecycle-'+stage+'-'+uuid.uuid4().hex[:7]
        self.api.create(dict(apiVersion='v1',kind='ConfigMap',metadata=dict(name=name+'-case'),immutable=True,data={'case.txt':name}))
        body=mixed.image_body(name,'native-probes')
        build=body['spec']['source']['build']
        build['definition']['inline']='FROM scratch\nARG TARGETARCH\nCOPY probes/probe-${TARGETARCH} /probe\nCOPY case/case.txt /case.txt\nRUN ["/probe", "build"]\nENTRYPOINT ["/probe"]\n'
        build['inputs']['volumes'].append(dict(name='case',configMap=dict(name=name+'-case')))
        build['inputs']['volumeMounts']=[dict(name='source',mountPath='/workspace/context/probes',readOnly=True),dict(name='case',mountPath='/workspace/context/case',readOnly=True)]
        image=self.api.create(body)
        self.api.wait(lambda:any(c.get('reason')=='AwaitingDemand' for c in (self.api.get('images.kimage.dev',name,self.app).get('status') or {}).get('conditions',[])),name+' waits for actual consumers')
        self.consume(name,expected,architecture,unknown=architecture is None)
        image=self.api.wait(lambda:(value if mixed.is_ready(value) else None) if (value:=self.api.get('images.kimage.dev',name,self.app)) else None,name+' native build and publication')
        children=self.api.get('images.kimage.dev',namespace=self.app,selector='kimage.dev/platform-parent='+image['metadata']['uid'])['items']
        releases=self.api.get('imagereleases.kimage.dev',namespace=self.app)['items']
        demands=[d for d in self.api.get('imagedemands.kimage.dev',namespace=self.system)['items'] if d['spec']['imageUid']==image['metadata']['uid']]
        require(len(demands)==1,'Expected one fresh demand')
        mixed.assert_variants(image,children,releases,demands[0],expected)
        self.images.append((name,expected,architecture))
        self.check(stage+': fresh '+('unknown' if architecture is None else architecture)+' placement built and ran '+','.join(sorted(expected)))
        return image

    def consume(self,image,expected,architecture=None,unknown=False):
        if architecture is None and len(expected)==1 and not unknown:architecture=next(iter(expected))
        name='read-'+uuid.uuid4().hex[:9]
        body=mixed.deployment_body(image,self.app,architecture)
        body['metadata']['name']=name
        body['spec']['replicas']=len(expected)
        labels={'kimage.dev/lifecycle-consumer':name}
        body['spec']['selector']['matchLabels']=labels
        body['spec']['template']['metadata']['labels']=labels
        if not architecture:
            body['spec']['template']['spec']['affinity']['podAntiAffinity']['requiredDuringSchedulingIgnoredDuringExecution'][0]['labelSelector']['matchLabels']=labels
        self.api.create(body);self.consumers.append(name)
        def executed():
            pods=self.api.get('pods',namespace=self.app,selector='kimage.dev/lifecycle-consumer='+name)['items']
            if len(pods)!=len(expected) or not all(any(c['type']=='Ready' and c['status']=='True' for c in p.get('status',{}).get('conditions',[])) for p in pods):return None
            seen=set()
            for pod in pods:
                arch=self.nodes[pod['spec']['nodeName']]['status']['nodeInfo']['architecture']
                require('@sha256:' in pod['spec']['containers'][0]['image'],'Consumer was not pinned')
                text=self.api.call(['logs',pod['metadata']['name'],'-n',self.app,'-c','app','--tail=2']).stdout.strip()
                require(text=='linux/'+arch,'Native built artifact did not execute correctly')
                seen.add(arch)
            require(seen==expected,'Unexpected consumer platforms');return True
        self.api.wait(executed,'native retained workload '+image)

    def retained(self):
        pvc=self.api.get('pvc',self.id+'-data',self.system)
        pv=self.api.get('pv',pvc['spec']['volumeName'])
        secrets={s['metadata']['name']:dict(uid=s['metadata']['uid'],sha256=hashlib.sha256(json.dumps(s.get('data',{}),sort_keys=True).encode()).hexdigest()) for s in self.api.get('secrets',namespace=self.system)['items'] if s['metadata'].get('labels',{}).get('kimage.dev/installation')==self.id}
        def objects(kind):
            return {o['metadata']['name']:dict(uid=o['metadata']['uid'],spec=o['spec']) for o in self.api.get(kind,namespace=self.app)['items']}
        return dict(pvcUid=pvc['metadata']['uid'],pvUid=pv['metadata']['uid'],secrets=secrets,images=objects('images.kimage.dev'),releases=objects('imagereleases.kimage.dev'))

    def assert_retained(self,before):
        after=self.retained()
        require(all(before[key]==after[key] for key in ['pvcUid','pvUid']),'Registry storage identity changed')
        for kind in ['secrets','images','releases']:
            require(all(after[kind].get(name)==value for name,value in before[kind].items()),'Retained '+kind+' changed')

    def operation(self):
        value=self.api.get('configmap',self.id+'-operation',self.system)
        return json.loads(value['data']['operation.json']) if value else None

    def assert_removed(self):
        def absent():
            for kind in ['pods','deployments','jobs','configmaps','secrets','services','networkpolicies','roles','rolebindings','serviceaccounts']:
                if self.api.get(kind,namespace=self.system,selector='kimage.dev/installation='+self.id)['items']:return False
            return True
        self.api.wait(absent,'all disposable system resources removed, including controller-created resources and operation journal')

    def transition(self,version,interrupt=False):
        command='upgrade' if version=='0.5.0' else 'rollback'
        catalog=Path(self.config['catalog']) if version=='0.5.0' else self.previous/'component-images.json'
        chart=Path(self.config['chart']) if version=='0.5.0' else self.previous/'kimage-0.4.1.tgz'
        args=[self.installer,command,'--to',version,'--namespace',self.system,'--installation',self.id,'--image-catalog',catalog,'--chart',chart]
        old_operation=self.operation()
        self.run_command(args+['--check'])
        require(self.operation()==old_operation,'Read-only preflight wrote an operation')
        before=self.retained()
        if interrupt:
            path=self.output/(command+'-interrupted.log')
            with path.open('w') as output:
                process=subprocess.Popen([str(a) for a in args],env=self.environment,stdout=output,stderr=subprocess.STDOUT,start_new_session=True)
                deadline=time.monotonic()+180
                try:
                    while True:
                        if process.poll() is not None:
                            detail=path.read_text()[-3000:]
                            raise RuntimeError('Operation exited '+str(process.returncode)+' before interruption; fixture must prove interruption. '+detail)
                        record=self.operation()
                        if record and record['phase']=='Applying':
                            if self.config['mode']=='cli':
                                policy=self.api.get('configmap',self.id+'-component-images',self.system)
                                ready=json.loads(policy['data']['images.json'])['release']==version
                            else:
                                history=json.loads(self.run_command(['helm','history',self.id,'--namespace',self.system,'--output','json']).stdout)
                                ready=history[-1]['status'].startswith('pending-')
                            if ready:break
                        require(time.monotonic()<deadline,'Did not observe an interruptible phase');time.sleep(.3)
                    os.killpg(process.pid,signal.SIGKILL);process.wait(timeout=15)
                finally:
                    if process.poll() is None:os.killpg(process.pid,signal.SIGKILL);process.wait(timeout=15)
            require(self.operation()['phase']!='Complete','Interrupted operation incorrectly marked complete')
            self.check(command+' interrupted with a persisted incomplete operation')
            time.sleep(31)
        self.run_command(args)
        require(self.operation()['phase']=='Complete','Operation did not verify completion')
        self.assert_retained(before)
        self.assert_images(version,self.target if version=='0.5.0' else json.loads((self.previous/'component-images.json').read_bytes()))
        self.check(command+' completed with retained registry, credentials, Images and releases')

    def diagnostics(self,arch):
        name='failed-'+arch+'-'+uuid.uuid4().hex[:7]
        body=mixed.image_body(name,'native-probes')
        body['spec']['platforms']=[dict(os='linux',architecture=arch)]
        body['spec']['source']['build']['definition']['inline']='FROM scratch\nARG TARGETARCH\nCOPY probe-${TARGETARCH} /probe\nRUN ["/probe", "diagnose"]\n'
        image=self.api.create(body)
        def attempts():
            return json.loads(self.run_command([self.installer,'builds',name,'-n',self.app]).stdout)
        active=self.api.wait(lambda:attempts().get('active'),'native failed build starts')
        attempt=active[0]['attempt']
        output=self.run_command([self.installer,'logs',name,'-n',self.app,'--platform','linux/'+arch,'--attempt',attempt,'--follow'],timeout=180)
        retained=self.api.wait(lambda:attempts().get('retained'),'private failed attempt survives Pod cleanup')
        archived=self.run_command([self.installer,'logs',name,'-n',self.app,'--attempt',attempt])
        log_assertions.verify(output.stdout, archived.stdout, retained, attempt)
        secrets=self.api.get('secrets',namespace=self.app,selector='kimage.dev/build-log='+image['metadata']['uid'])['items']
        require(len(secrets)==1 and secrets[0]['immutable'] and secrets[0]['metadata']['labels'].get('kimage.dev/build-state')==image['metadata']['uid'],'Private output lacks the predecessor reserved-input marker')
        public=json.dumps(self.api.get('images.kimage.dev',namespace=self.app))+json.dumps(self.api.get('imagereleases.kimage.dev',namespace=self.app))
        require('kimage-diagnostic-output-' not in public and 'kimage-diagnostic-failure-end' not in public,'Raw failure output leaked into public metadata')
        self.private_logs.append((arch,secrets[0]['metadata']['name']))
        self.check('native '+arch+' failure streams and retains a bounded private log tail')

    def check_rollback_log_privacy(self):
        for arch,secret in self.private_logs:
            name='reserved-'+arch+'-'+uuid.uuid4().hex[:7]
            body=mixed.image_body(name,'native-probes')
            body['spec']['platform']=dict(os='linux',architecture=arch)
            build=body['spec']['source']['build']
            # Keep the ordinary executable context valid. A Secret must mount
            # below the sensitive root; otherwise syntax validation would reject
            # this fixture before it reaches reserved-input authorization.
            build['definition']['inline']='FROM scratch\nARG TARGETARCH\nCOPY probe-${TARGETARCH} /probe\nRUN --mount=type=secret,id=diagnostic ["/probe", "build"]\n'
            build['inputs']['volumes'].append(dict(name='diagnostic',secret=dict(name=secret)))
            build['inputs']['volumeMounts'].append(dict(name='diagnostic',mountPath='/run/kimage-build-secrets/diagnostic',readOnly=True))
            build['buildSecrets']=[dict(id='diagnostic',sourceFile='/run/kimage-build-secrets/diagnostic/attempt.json')]
            image=self.api.create(body)
            def refused():
                value=self.api.get('images.kimage.dev',name,self.app)
                conditions=value.get('status',{}).get('conditions',[])
                reasons=[c.get('reason','') for c in conditions]
                require(not any(r.startswith(('Invalid','Unsupported')) for r in reasons),'Reserved-input fixture failed validation before authorization: '+','.join(reasons))
                require(not mixed.is_ready(value),'Private diagnostic input was incorrectly accepted')
                return 'ReservedControllerInputDenied' in reasons
            self.api.wait(refused,'published predecessor rejects private log Secret as an input')
            require(not any(r['spec']['imageUid']==image['metadata']['uid'] for r in self.api.get('imagereleases.kimage.dev',namespace=self.app)['items']),'Predecessor exported private diagnostic data')
        self.check('published predecessor preserves private log input protection after rollback')

    def verify_graphs(self,stage):
        releases=self.api.get('imagereleases.kimage.dev',namespace=self.app)['items']
        require(releases and all(r.get('status',{}).get('verified') for r in releases),'Expected verified retained releases')
        requests=[dict(repository=self.app+'/'+r['spec']['imageUid'],root=r['spec']['root']) for r in releases]
        name=self.id+'-graph-'+uuid.uuid4().hex[:8]
        labels={'kimage.dev/lifecycle-graph':name}
        request=dict(apiVersion='v1',kind='ConfigMap',metadata=dict(name=name,namespace=self.system,labels=labels),immutable=True,data={'requests.json':json.dumps(requests,separators=(',',':'))})
        security=dict(runAsUser=65532,runAsGroup=65532,runAsNonRoot=True,allowPrivilegeEscalation=False,readOnlyRootFilesystem=True,capabilities=dict(drop=['ALL']))
        job=dict(apiVersion='batch/v1',kind='Job',metadata=dict(name=name,namespace=self.system,labels=labels),spec=dict(backoffLimit=0,activeDeadlineSeconds=600,template=dict(metadata=dict(labels=labels),spec=dict(automountServiceAccountToken=False,serviceAccountName=self.id+'-serving',nodeName=self.primary,restartPolicy='Never',securityContext=dict(fsGroup=65532,seccompProfile=dict(type='RuntimeDefault')),containers=[dict(name='verify',image=self.target['repository']+'/input@'+self.target['components']['input']['digest'],imagePullPolicy='IfNotPresent',command=['/kimage-input','verify-registry','--request','/request/requests.json','--origin','https://'+self.id+'-registry.'+self.system+'.svc:5443/','--credentials','/credentials'],securityContext=security,resources=dict(requests=dict(cpu='50m',memory='64Mi'),limits=dict(cpu='1',memory='512Mi',**{'ephemeral-storage':'2Gi'})),volumeMounts=[dict(name='request',mountPath='/request',readOnly=True),dict(name='credentials',mountPath='/credentials',readOnly=True),dict(name='scratch',mountPath='/tmp')])],volumes=[dict(name='request',configMap=dict(name=name,defaultMode=292)),dict(name='credentials',secret=dict(secretName=self.id+'-proxy-credentials',defaultMode=292)),dict(name='scratch',emptyDir=dict(sizeLimit='2Gi'))]))))
        self.api.call(['create','--raw','/api/v1/namespaces/'+self.system+'/configmaps?fieldValidation=Strict','-f','-'],request)
        try:
            self.api.call(['create','--raw','/apis/batch/v1/namespaces/'+self.system+'/jobs?fieldValidation=Strict','-f','-'],job)
            def finished():
                current=self.api.get('job',name,self.system)
                require(not current.get('status',{}).get('failed'),'Registry full graph verification failed')
                return current.get('status',{}).get('succeeded')
            self.api.wait(finished,'cluster-local authenticated full OCI graph readback')
            receipt=json.loads(self.api.call(['logs','job/'+name,'-n',self.system,'-c','verify']).stdout)
            require(receipt['complete'] and receipt['verifiedReleases']==len(releases),'Incomplete retained OCI graph readback')
            self.report.setdefault('registryReadback',[]).append(dict(stage=stage,**receipt))
            self.check(stage+': all retained OCI graphs verified locally with authenticated TLS, sizes, SHA-256 and DiffIDs')
        finally:
            self.api.call(['delete','job',name,'-n',self.system,'--ignore-not-found','--cascade=foreground','--wait=true'],ok=False)
            self.api.call(['delete','configmap',name,'-n',self.system,'--ignore-not-found'],ok=False)

    def run(self):
        try:
            self.preflight();self.install_previous()
            self.build('before')
            self.verify_graphs('before-upgrade')
            self.transition('0.5.0',True)
            self.installation_diagnostics()
            for image,expected,architecture in list(self.images):self.consume(image,expected,architecture)
            self.build('after')
            for architecture in sorted(self.architectures):
                self.build('explicit-'+architecture,architecture)
                self.diagnostics(architecture)
            if self.config.get('nodeChanges'):self.node_changes()
            self.installation_diagnostics()
            self.transition('0.4.1',True)
            self.build('rolled-back')
            for image,expected,architecture in self.images:self.consume(image,expected,architecture)
            self.check('published 0.4.1 consumes releases produced before and after the upgrade')
            self.check_rollback_log_privacy()
            self.verify_graphs('after-rollback')
            retained=self.retained()
            for name in self.consumers:self.api.call(['delete','deployment',name,'-n',self.app,'--wait=true'])
            if self.config['mode']=='cli':self.run_command([self.installer,'uninstall','--namespace',self.system,'--installation',self.id])
            else:self.run_command(['helm','uninstall',self.id,'--namespace',self.system,'--wait','--timeout','5m'])
            # Secrets are disposable; catalog and registry identities are retained.
            after=self.retained();require(all(after[k]==retained[k] for k in ['pvcUid','pvUid','images','releases']),'Uninstall changed retained data')
            self.assert_removed()
            self.check('uninstall preserves registry data, Image identities and release contracts')
            self.install_previous()
            self.verify_graphs('after-uninstall-reinstall')
            if self.config['mode']=='cli':self.run_command([self.installer,'uninstall','--namespace',self.system,'--installation',self.id])
            else:self.run_command(['helm','uninstall',self.id,'--namespace',self.system,'--wait','--timeout','5m'])
            after=self.retained();require(all(after[k]==retained[k] for k in ['pvcUid','pvUid','images','releases']),'Final removal changed retained data')
            self.assert_removed()
            self.report['complete']=True
        finally:
            self.restore_labels()
            if self.private_config:Path(self.private_config.name).unlink(missing_ok=True)
            (self.output/'results.json').write_text(json.dumps(self.report,indent=2)+'\n')

    def restore_labels(self):
        for name,old in list(self.saved_labels.items()):self.set_node_label(name,old)
        self.saved_labels.clear()
        (self.output/'node-labels.private.json').unlink(missing_ok=True)

    def set_node_label(self,name,value):
        for attempt in range(3):
            node=self.api.get('node',name)
            require(node['metadata']['uid']==self.nodes[name]['metadata']['uid'],'Node identity changed during fixture label update')
            self.saved_labels.setdefault(name,node['metadata'].get('labels',{}).get(self.label))
            (self.output/'node-labels.private.json').write_text(json.dumps(dict(label=self.label,original=self.saved_labels)))
            patch=dict(metadata=dict(uid=node['metadata']['uid'],resourceVersion=node['metadata']['resourceVersion'],labels={self.label:value}))
            result=self.api.call(['patch','node',name,'--type=merge','--patch',json.dumps(patch),'-o','name'],ok=False)
            if result.returncode==0:return
            require('Conflict' in result.stderr and attempt<2,'Fixture Node label update failed')

    def set_policy(self,selector,pins):
        policy=dict(schema=1,nodeSelector=selector,workerPins=pins)
        for attempt in range(3):
            current=self.api.get('configmap',self.id+'-node-policy',self.system)
            require(current['metadata'].get('labels',{}).get('kimage.dev/installation')==self.id,'Node policy ownership changed')
            current['data']['policy.json']=json.dumps(policy,separators=(',',':'))
            result=self.api.call(['replace','--raw','/api/v1/namespaces/'+self.system+'/configmaps/'+self.id+'-node-policy?fieldValidation=Strict','-f','-'],current,ok=False)
            if result.returncode==0:return
            require('Conflict' in result.stderr and attempt<2,'Fixture node policy update failed')

    def membership(self,expected,workers=None):
        maps=self.api.get('configmaps',namespace=self.system,selector='kimage.dev/installation='+self.id+',kimage.dev/role=node-endpoint')['items']
        endpoints=[json.loads(m['data']['endpoint.json']) for m in maps]
        if {e['node'] for e in endpoints}!=expected:return None
        for endpoint in endpoints:
            require(endpoint['nodeUid']==self.nodes[endpoint['node']]['metadata']['uid'],'Serving approval has a stale Node UID')
            name=self.id+'-n-'+hashlib.sha256(endpoint['nodeUid'].encode()).hexdigest()[:16]+'-proxy'
            pod=self.api.get('pod',name,self.system)
            if not pod or not any(c['type']=='Ready' and c['status']=='True' for c in pod.get('status',{}).get('conditions',[])):return None
        record=json.loads(self.api.get('configmap',self.id+'-installation',self.system)['data']['installation.json'])
        platforms={self.nodes[n]['status']['nodeInfo']['architecture'] for n in expected}
        if set(record['options']['workerNodes'])!=platforms:return None
        for arch in self.architectures:
            pod=self.api.get('pod',self.id+'-builder-'+arch,self.system)
            catalog=self.api.get('configmap',self.id+'-builders-'+arch,self.system)
            if arch not in platforms:
                if pod or catalog:return None
                continue
            if not pod or not catalog or not any(c['type']=='Ready' and c['status']=='True' for c in pod.get('status',{}).get('conditions',[])):return None
            selected=pod['spec']['nodeName']
            if selected not in expected or workers and selected!=workers[arch]:return None
            if json.loads(catalog['data']['catalog.json'])['builders']['default']['node']!=selected:return None
            if pod['metadata'].get('annotations',{}).get('kimage.dev/node-uid')!=self.nodes[selected]['metadata']['uid']:return None
        status=self.api.get('configmap',self.id+'-node-status',self.system)
        if not status or json.loads(status['data']['status.json'])['reason']!='Ready':return None
        return True

    def node_changes(self):
        self.label='kimage.dev/lifecycle-'+uuid.uuid4().hex[:12]
        original=json.loads(self.api.get('configmap',self.id+'-node-policy',self.system)['data']['policy.json'])
        require(original==dict(schema=1,nodeSelector={},workerPins={}),'Fresh default node policy required')
        record=json.loads(self.api.get('configmap',self.id+'-installation',self.system)['data']['installation.json'])
        original_workers=record['options']['workerNodes']
        before=self.retained()
        arm=next(n for n,node in self.nodes.items() if node['status']['nodeInfo']['architecture']=='arm64')
        spare=next(n for n,node in self.nodes.items() if n!=self.primary and node['status']['nodeInfo']['architecture']=='amd64')
        try:
            self.set_node_label(self.primary,'enabled')
            self.set_policy({self.label:'enabled'}, {})
            self.api.wait(lambda:self.membership({self.primary}),'revoke excluded native membership and fence arm64 worker')
            self.build('nodes-amd-only',available={'amd64'})
            self.set_node_label(arm,'enabled')
            self.api.wait(lambda:self.membership({self.primary,arm}),'new arm64 worker and UID-bound serving approval')
            self.build('nodes-mixed')
            self.set_node_label(spare,'enabled')
            self.api.wait(lambda:self.membership({self.primary,arm,spare}),'new eligible amd64 serving proxy')
            scope=self.id+'-n-'+hashlib.sha256(self.nodes[spare]['metadata']['uid'].encode()).hexdigest()[:16]
            proxy=self.api.get('pod',scope+'-proxy',self.system)
            installation_uid=self.api.get('configmap',self.id+'-installation',self.system)['metadata']['uid']
            require(any(o['kind']=='ConfigMap' and o['uid']==installation_uid for o in proxy['metadata'].get('ownerReferences',[])),'Dynamic node resource is not garbage-collectable with its installation')
            previous_uid=self.api.get('pod',self.id+'-builder-amd64',self.system)['metadata']['uid']
            self.set_policy({self.label:'enabled'},{'amd64':spare})
            self.api.wait(lambda:self.membership({self.primary,arm,spare},dict(amd64=spare,arm64=arm)),'explicit worker placement fences and replaces old worker')
            require(self.api.get('pod',self.id+'-builder-amd64',self.system)['metadata']['uid']!=previous_uid,'Worker replacement did not change Pod identity')
            self.assert_retained(before)
            self.check('real mixed-node eligibility, new architecture, UID-bound proxies and pinned worker replacement preserve storage')
        finally:
            self.set_policy({},original_workers)
            self.api.wait(lambda:self.membership(set(self.nodes),original_workers),'restore original worker placement before compatibility rollback')
            self.set_policy(original['nodeSelector'],original['workerPins'])
            self.restore_labels()
        self.api.wait(lambda:self.membership(set(self.nodes),original_workers),'restored automatic membership')


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--run',action='store_true',help='Explicit authorization to create the isolated fixture')
    parser.add_argument('--config',type=Path,required=True)
    args=parser.parse_args()
    require(args.run,'Opt in with --run; this test never uses an implicit cluster context')
    Upgrade(json.loads(args.config.read_bytes())).run()


if __name__=='__main__':main()

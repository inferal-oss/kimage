#!/usr/bin/env python3
"""Explicit Argo CD acceptance. Uses an existing controller and isolated fixtures.

Never modifies other Applications, restarts shared Argo controllers, deletes Nodes
or deletes retained registry data. Only fixture Node labels are changed and restored.
"""
import argparse
import copy
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import time

ROOT=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location('mixed',ROOT/'scripts/e2e-mixed-cluster.py')
mixed=importlib.util.module_from_spec(spec);spec.loader.exec_module(mixed)

def require(value,message):
    if not value:raise RuntimeError(message)

class Argo:
    def __init__(self,config):
        required={'kubeconfig','context','namespace','systemNamespace','installation','probes','output','catalog','installer','scope','port','registryNode','runtimeProfile'}
        require(required<=config.keys() and not config.keys()-required-{'sourceRevision','chartRepository','nodeChanges','kubectlCommand','timeoutSeconds'},'Invalid Argo fixture configuration')
        require(config['scope'] in ('namespace','cluster'),'Invalid fixture scope')
        require(bool(config.get('sourceRevision'))!=bool(config.get('chartRepository')),'Select an exact Git source revision or public chart repository')
        self.config=config;self.app=config['namespace'];self.system=config['systemNamespace'];self.id=config['installation']
        require(self.app.startswith('kimage-') and self.system.startswith('kimage-') and self.id.startswith('kimage'),'Isolated kimage fixture names required')
        self.output=Path(config['output']).resolve();self.output.mkdir(parents=True,exist_ok=False)
        base={k:config[k] for k in ['kubeconfig','context','namespace','systemNamespace','installation','probes']}
        base['output']=str(self.output/'builds');base['timeoutSeconds']=config.get('timeoutSeconds',900)
        self.api=mixed.MixedCluster(base)
        self.api.command=config.get('kubectlCommand',['kubectl'])+self.api.command[1:]
        self.catalog=json.loads(Path(config['catalog']).read_text());require(self.catalog['release']=='0.5.1','Exact 0.5.1 catalog required')
        self.nodes={};self.labels={};self.report=dict(schema=1,test='argocd-installation',release='0.5.1',scope=config['scope'],checks=[],complete=False)
        self.report['harnessSha256']='sha256:'+hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
        self.report['catalogSha256']='sha256:'+hashlib.sha256(Path(config['catalog']).read_bytes()).hexdigest()
        self.application=None

    def check(self,message):
        self.report['checks'].append(message);self.save();print('PASS: '+message,flush=True)
    def save(self):
        (self.output/'results.json').write_text(json.dumps(self.report,indent=2)+'\n')
    def raw(self,method,path,body):
        return json.loads(self.api.call([method,'--raw',path,'-f','-'],body).stdout)
    def patch(self,kind,name,body,namespace=None):
        args=['patch',kind,name]+(['-n',namespace] if namespace else [])+['--type=merge','-p',json.dumps(body),'-o','json']
        return json.loads(self.api.call(args).stdout)
    def application_state(self):return self.api.get('applications.argoproj.io',self.id,'argocd')
    def ready(self):
        value=self.application_state()
        if not value:return False
        status=value.get('status',{})
        return status.get('health',{}).get('status')=='Healthy' and status.get('sync',{}).get('status')=='Synced' and status.get('operationState',{}).get('phase')=='Succeeded'
    def wait_ready(self):self.api.wait(self.ready,'Argo Application Healthy and Synced: '+self.id)
    def desired(self,changes):
        current=self.application_state();source=copy.deepcopy(current['spec']['source']);source['helm']['valuesObject'].update(changes)
        self.patch('applications.argoproj.io',self.id,dict(metadata=dict(resourceVersion=current['metadata']['resourceVersion']),spec=dict(source=source)),'argocd')
        return source
    def stop_sync(self):
        current=self.application_state()
        if current.get('status',{}).get('operationState',{}).get('phase')=='Running':
            self.patch('applications.argoproj.io',self.id,dict(status=dict(operationState=dict(phase='Terminating'))),'argocd')
            self.api.wait(lambda:self.application_state().get('status',{}).get('operationState',{}).get('phase') not in ('Running','Terminating'),'failed desired-state sync termination')
    def sync(self):
        old=self.application_state().get('status',{}).get('operationState',{}).get('finishedAt')
        self.patch('applications.argoproj.io',self.id,dict(operation=dict(sync=dict(prune=True))),'argocd')
        self.api.wait(lambda:self.ready() and self.application_state().get('status',{}).get('operationState',{}).get('finishedAt')!=old,'repeat sync completed')
    def restart(self,selector,namespace):
        pods=self.api.get('pods',namespace=namespace,selector=selector)['items'];require(len(pods)==1,'Expected exactly one isolated controller Pod')
        pod=pods[0]
        self.raw('delete','/api/v1/namespaces/'+namespace+'/pods/'+pod['metadata']['name'],dict(apiVersion='v1',kind='DeleteOptions',preconditions=dict(uid=pod['metadata']['uid'])))
    def fingerprint(self):
        result={}
        for kind,name in [('configmap',self.id+'-installation'),('pvc',self.id+'-data')]+[('secret',self.id+'-'+role+'-credentials') for role in ['registry','admission','builder']]:
            obj=self.api.get(kind,name,self.system);require(obj is not None,'Missing retained installation identity')
            result[kind+'/'+name]=dict(uid=obj['metadata']['uid'])
            if kind=='secret':result[kind+'/'+name]['sha256']='sha256:'+hashlib.sha256(json.dumps(obj['data'],sort_keys=True).encode()).hexdigest()
        result['releases']={r['metadata']['name']:r['metadata']['uid'] for r in self.api.get('imagereleases.kimage.dev',namespace=self.app)['items']}
        return result
    def refused_target(self):
        deployment=self.api.get('deployment',self.id+'-provisioner',self.system)
        wanted=deployment['spec']['template']['metadata']['annotations']['checksum/configuration']
        pods=self.api.get('pods',namespace=self.system,selector='kimage.dev/provisioner='+self.id)['items']
        for pod in pods:
            if pod['metadata'].get('annotations',{}).get('checksum/configuration')!=wanted:continue
            if pod['status'].get('phase')!='Running':continue
            result=self.api.call(['logs',pod['metadata']['name'],'-n',self.system,'-c','provisioner','--tail=10'],ok=False)
            if result.returncode==0 and 'UnsupportedReleaseTransition' in result.stdout:
                return not any(c['type']=='Ready' and c['status']=='True' for c in pod['status'].get('conditions',[]))
        return False
    def membership(self,names,workers):
        pods=self.api.get('pods',namespace=self.system,selector='kimage.dev/installation='+self.id)['items']
        proxies={p['spec']['nodeName'] for p in pods if p['metadata'].get('labels',{}).get('kimage.dev/role')=='node-proxy' and p['status'].get('phase')=='Running' and any(c['type']=='Ready' and c['status']=='True' for c in p['status'].get('conditions',[]))}
        native={p['metadata']['labels'].get('kimage.dev/worker-architecture') for p in pods if p['metadata'].get('labels',{}).get('kimage.dev/role')=='builder' and any(c['type']=='Ready' and c['status']=='True' for c in p['status'].get('conditions',[]))}
        return proxies==set(names) and native==set(workers)
    def label(self,node,value):
        self.patch('node',node,dict(metadata=dict(labels={self.label_key:value})))
    def node_changes(self):
        self.label_key='kimage.dev/argo-e2e-'+self.id
        self.labels={n:nv['metadata'].get('labels',{}).get(self.label_key) for n,nv in self.nodes.items()}
        for name in self.nodes:self.label(name,'selected')
        policy=dict(schema=1,nodeSelector={self.label_key:'selected'},workerPins={})
        self.desired(dict(nodePolicy=policy));self.wait_ready()
        arm=next(n for n,v in self.nodes.items() if v['status']['nodeInfo']['architecture']=='arm64')
        self.label(arm,None)
        remaining=set(self.nodes)-{arm}
        self.api.wait(lambda:self.membership(remaining,{'amd64'}),'arm64 membership retired with worker fencing')
        self.wait_ready()
        self.label(arm,'selected')
        self.api.wait(lambda:self.membership(self.nodes,{'amd64','arm64'}),'newly available arm64 native worker and proxy')
        self.wait_ready()
        other=next(n for n in self.nodes if n not in (arm,self.config['registryNode']))
        self.label(other,None)
        self.api.wait(lambda:self.membership(set(self.nodes)-{other},{'amd64','arm64'}),'ordinary node proxy revoked')
        self.label(other,'selected')
        self.api.wait(lambda:self.membership(self.nodes,{'amd64','arm64'}),'ordinary node proxy restored')
        self.desired(dict(nodePolicy=dict(schema=1,nodeSelector={},workerPins={})));self.wait_ready()
        for name,value in self.labels.items():self.label(name,value)
        self.labels={}
        self.check('reversible node eligibility, lost/new native architecture and proxies remain Healthy and Synced')
    def create_application(self):
        path='/apis/argoproj.io/v1alpha1/namespaces/argocd/applications?fieldValidation=Strict'
        self.raw('create',path,self.application)
    def delete_application(self):
        value=self.application_state();require(value is not None,'Missing owned Application')
        self.raw('delete','/apis/argoproj.io/v1alpha1/namespaces/argocd/applications/'+self.id,dict(apiVersion='v1',kind='DeleteOptions',preconditions=dict(uid=value['metadata']['uid'])))
        self.api.wait(lambda:self.application_state() is None,'owned Application deletion')
        self.api.wait(lambda:self.api.get('deployment',self.id+'-provisioner',self.system) is None,'provisioner removed by Argo cascade')
    def run(self):
        try:
            require(self.application_state() is None,'Fresh Argo Application identity required')
            require(self.api.get('namespace',self.app) is None and self.api.get('namespace',self.system) is None,'Fresh fixture namespaces required')
            self.nodes={n['metadata']['name']:n for n in self.api.get('nodes')['items']}
            arches={n['status']['nodeInfo']['architecture'] for n in self.nodes.values()}
            require(arches<=mixed.ARCHITECTURES and arches,'Supported native architectures required')
            require(not self.config.get('nodeChanges') or arches=={'amd64','arm64'},'Node-change acceptance requires a real mixed cluster')
            for ns in [self.app,self.system]:self.raw('create','/api/v1/namespaces?fieldValidation=Strict',dict(apiVersion='v1',kind='Namespace',metadata=dict(name=ns)))
            values=dict(installation=self.id,scope=self.config['scope'],port=self.config['port'],runtimeProfile=self.config['runtimeProfile'],registryNode=self.config['registryNode'],persistence=dict(storageClass='local-path'),gitops=dict(enabled=True),images=dict(repository=self.catalog['repository'],digests={k:v['digest'] for k,v in self.catalog['components'].items()}))
            if self.config['scope']=='namespace':values['applicationNamespace']=self.app
            else:values['excludedNamespaces']=sorted(n['metadata']['name'] for n in self.api.get('namespaces')['items'] if n['metadata']['name'] not in (self.app,self.system) and not n['metadata']['name'].startswith('kube-'))
            source=dict(helm=dict(releaseName=self.id,valuesObject=values))
            if self.config.get('sourceRevision'):source.update(repoURL='https://github.com/inferal-oss/kimage.git',path='charts/kimage',targetRevision=self.config['sourceRevision'])
            else:source.update(repoURL=self.config['chartRepository'],chart='kimage',targetRevision='0.5.1')
            self.application=dict(apiVersion='argoproj.io/v1alpha1',kind='Application',metadata=dict(name=self.id,namespace='argocd',finalizers=['resources-finalizer.argocd.argoproj.io']),spec=dict(project='default',source=source,destination=dict(server='https://kubernetes.default.svc',namespace=self.system),syncPolicy=dict(automated=dict(prune=True,selfHeal=True),syncOptions=['CreateNamespace=true'])))
            self.report['source']=dict(repoURL=source['repoURL'],targetRevision=source['targetRevision'])
            self.create_application()
            self.api.wait(lambda:self.api.get('secret',self.id+'-registry-credentials',self.system),'first credentials persisted before initial provisioning interruption')
            self.restart('kimage.dev/provisioner='+self.id,self.system)
            self.wait_ready();self.check('fresh install resumes after provisioner interruption and becomes Healthy/Synced')
            identity=self.fingerprint()
            require(self.api.get('configmap',self.id+'-installation',self.system)['metadata']['annotations']['kimage.dev/manager']=='argocd','Wrong installation manager')
            self.sync();self.sync();require(self.fingerprint()==identity,'Repeated sync changed retained identities')
            self.check('repeated sync preserves credential bytes/UIDs, registry claim and installation identity')
            self.patch('deployment',self.id+'-provisioner',dict(spec=dict(replicas=0)),self.system)
            self.api.wait(lambda:self.api.get('deployment',self.id+'-provisioner',self.system)['spec']['replicas']==1,'Argo self-healing of direct desired resource')
            self.wait_ready();require(self.fingerprint()==identity,'Self-heal changed retained identities')
            self.restart('kimage.dev/installation='+self.id+',kimage.dev/role=operator',self.system)
            self.wait_ready();require(self.fingerprint()==identity,'Runtime restart changed retained identities')
            self.check('Argo self-heal and isolated runtime controller restart preserve installation')
            raw='{ "schema": 1, "buildPolicy": "OnDemand" }'
            self.patch('configmap',self.id+'-config',dict(data={'configuration.json':raw}),self.system)
            self.sync();require(self.api.get('configmap',self.id+'-config',self.system)['data']['configuration.json']==raw,'Default desired state overwrote administrator policy')
            self.check('null policy values preserve administrator configuration bytes')
            existing=self.api.get('secret',self.id+'-registry-credentials',self.system)
            self.raw('create','/api/v1/namespaces/'+self.system+'/secrets?fieldValidation=Strict',dict(apiVersion='v1',kind='Secret',metadata=dict(name=self.id+'-external-registry'),type='Opaque',data=existing['data']))
            self.desired(dict(gitops=dict(enabled=True,credentialSecrets=dict(registry=self.id+'-external-registry'))));self.wait_ready()
            require(self.fingerprint()==identity,'Existing credential reference changed identity')
            self.desired(dict(gitops=dict(enabled=True)));self.wait_ready()
            self.check('external credential Secret reference is resolved in cluster without material in desired state')
            if self.config.get('nodeChanges'):self.node_changes()
            # Reuse the native demand/publication/pull assertions. Single-node CI
            # uses its one real architecture; mixed acceptance requires both.
            mixed.ARCHITECTURES=arches
            self.api.preflight()
            for arch in [None]+sorted(arches):self.api.case(arch)
            self.api.report['complete']=True;self.api.save()
            self.check('fresh unknown and explicit placement cases build and execute only requested native platforms')
            identity=self.fingerprint()
            bad=copy.deepcopy(self.catalog);bad['release']='0.6.0'
            for role in ['operator','admission','input','exporter']:bad['components'][role]['version']='0.6.0'
            self.desired(dict(gitops=dict(enabled=True,targetCatalog=bad)))
            self.api.wait(self.refused_target,'unsupported target was observed, refused and remains unready')
            require(self.fingerprint()==identity,'Unsupported transition changed installation')
            require(json.loads(self.api.get('configmap',self.id+'-installation',self.system)['data']['installation.json'])['release']=='0.5.1','Unsupported transition changed release')
            self.stop_sync()
            self.desired(dict(gitops=dict(enabled=True)));self.wait_ready()
            self.check('unsupported runtime target is refused without changing credentials, data or installed version')
            self.delete_application();require(self.fingerprint()==identity,'Application deletion removed retained identity or release')
            self.check('Application cascade leaves runtime, registry data, credentials and release UIDs intact')
            # Recreate the exact desired Application after deletion.
            self.application['spec']['source']=source
            self.create_application();self.wait_ready();require(self.fingerprint()==identity,'Reinstall changed retained identities')
            self.check('Application reinstall resumes the retained installation with unchanged release and storage identities')
            self.delete_application()
            environment=dict(os.environ,KUBECONFIG=self.config['kubeconfig'])
            result=subprocess.run([self.config['installer'],'uninstall','--namespace',self.system,'--installation',self.id],env=environment,text=True,capture_output=True,timeout=900)
            require(result.returncode==0,'Explicit runtime uninstall failed; inspect private local output')
            require(self.api.get('pvc',self.id+'-data',self.system)['metadata']['uid']==identity['pvc/'+self.id+'-data']['uid'],'Uninstall removed registry claim')
            require({r['metadata']['name']:r['metadata']['uid'] for r in self.api.get('imagereleases.kimage.dev',namespace=self.app)['items']}==identity['releases'],'Uninstall removed releases')
            self.check('explicit runtime uninstall after Application removal preserves registry claim and releases')
            self.report['complete']=True
        except Exception as error:
            self.report['failure']=str(error);raise
        finally:
            for name,value in self.labels.items():self.label(name,value)
            for name,uid in reversed(self.api.deployments):
                self.api.call(['delete','--raw','/apis/apps/v1/namespaces/'+self.app+'/deployments/'+name,'-f','-'],dict(apiVersion='v1',kind='DeleteOptions',propagationPolicy='Foreground',preconditions=dict(uid=uid)),ok=False)
            self.report['retainedNamespaces']=[self.app,self.system];self.save()

def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--run',action='store_true');p.add_argument('--config',type=Path,required=True);a=p.parse_args()
    require(a.run,'Opt in with --run before accessing any configuration or cluster')
    Argo(json.loads(a.config.read_text())).run()
if __name__=='__main__':main()

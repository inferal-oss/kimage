#!/usr/bin/env python3
"""Shared native pull and authenticated complete OCI graph acceptance assertions."""
import json
import uuid

def require(value,message):
    if not value:raise RuntimeError(message)

def native_components(api,system,installation,nodes,architectures,version,catalog):
    record=json.loads(api.get('configmap',installation+'-installation',system)['data']['installation.json'])
    require(record['release']==version,'Wrong installed release')
    def active_pods():
        return [p for p in api.get('pods',namespace=system)['items'] if p['metadata'].get('labels',{}).get('kimage.dev/installation')==installation and not p['metadata'].get('deletionTimestamp') and p.get('status',{}).get('phase') not in ('Succeeded','Failed')]
    def pulled():
        pods=active_pods()
        return bool(pods) and all(p.get('spec',{}).get('nodeName') and {s['name'] for s in p.get('status',{}).get('containerStatuses',[]) if s.get('imageID') and s.get('ready')} >= {c['name'] for c in p['spec']['containers']} for p in pods)
    api.wait(pulled,'native container image identities and readiness after transition')
    own=active_pods()
    observed=set()
    identities=[]
    pins={catalog['repository']+'/'+role+'@'+entry['digest'] for role,entry in catalog['components'].items()}
    for pod in own:
        arch=nodes[pod['spec']['nodeName']]['status']['nodeInfo']['architecture']
        require(arch in architectures,'Non-native fixture placement')
        for status in pod.get('status',{}).get('containerStatuses',[]):
            require(status.get('imageID'),'Runtime did not report an image identity')
            identities.append(dict(role=pod['metadata'].get('labels',{}).get('kimage.dev/role'),architecture=arch,container=status['name'],imageId=status['imageID']))
        for container in pod['spec']['containers']+pod['spec'].get('initContainers',[]):
            require(container['image'] in pins,'Unpinned component image')
        if pod['metadata'].get('labels',{}).get('kimage.dev/role') in ('builder','node-proxy'):observed.add(arch)
    require(observed==architectures,'Components did not run on every native architecture')
    return dict(release=version,identities=identities)


def verify_graphs(api,namespace,system,installation,node,catalog):
    releases=api.get('imagereleases.kimage.dev',namespace=namespace)['items']
    require(releases and all(r.get('status',{}).get('verified') for r in releases),'Expected verified retained releases')
    requests=[dict(repository=namespace+'/'+r['spec']['imageUid'],root=r['spec']['root']) for r in releases]
    name=installation+'-graph-'+uuid.uuid4().hex[:8]
    labels={'kimage.dev/lifecycle-graph':name}
    request=dict(apiVersion='v1',kind='ConfigMap',metadata=dict(name=name,namespace=system,labels=labels),immutable=True,data={'requests.json':json.dumps(requests,separators=(',',':'))})
    security=dict(runAsUser=65532,runAsGroup=65532,runAsNonRoot=True,allowPrivilegeEscalation=False,readOnlyRootFilesystem=True,capabilities=dict(drop=['ALL']))
    job=dict(apiVersion='batch/v1',kind='Job',metadata=dict(name=name,namespace=system,labels=labels),spec=dict(backoffLimit=0,activeDeadlineSeconds=600,template=dict(metadata=dict(labels=labels),spec=dict(automountServiceAccountToken=False,serviceAccountName=installation+'-serving',nodeName=node,restartPolicy='Never',securityContext=dict(fsGroup=65532,seccompProfile=dict(type='RuntimeDefault')),containers=[dict(name='verify',image=catalog['repository']+'/input@'+catalog['components']['input']['digest'],imagePullPolicy='IfNotPresent',command=['/kimage-input','verify-registry','--request','/request/requests.json','--origin','https://'+installation+'-registry.'+system+'.svc:5443/','--credentials','/credentials'],securityContext=security,resources=dict(requests=dict(cpu='50m',memory='64Mi'),limits=dict(cpu='1',memory='512Mi',**{'ephemeral-storage':'2Gi'})),volumeMounts=[dict(name='request',mountPath='/request',readOnly=True),dict(name='credentials',mountPath='/credentials',readOnly=True),dict(name='scratch',mountPath='/tmp')])],volumes=[dict(name='request',configMap=dict(name=name,defaultMode=292)),dict(name='credentials',secret=dict(secretName=installation+'-proxy-credentials',defaultMode=292)),dict(name='scratch',emptyDir=dict(sizeLimit='2Gi'))]))))
    api.call(['create','--raw','/api/v1/namespaces/'+system+'/configmaps?fieldValidation=Strict','-f','-'],request)
    try:
        api.call(['create','--raw','/apis/batch/v1/namespaces/'+system+'/jobs?fieldValidation=Strict','-f','-'],job)
        def finished():
            current=api.get('job',name,system)
            require(not current.get('status',{}).get('failed'),'Registry full graph verification failed')
            return current.get('status',{}).get('succeeded')
        api.wait(finished,'cluster-local authenticated full OCI graph readback')
        receipt=json.loads(api.call(['logs','job/'+name,'-n',system,'-c','verify']).stdout)
        require(receipt['complete'] and receipt['verifiedReleases']==len(releases),'Incomplete retained OCI graph readback')
        return receipt
    finally:
        api.call(['delete','job',name,'-n',system,'--ignore-not-found','--cascade=foreground','--wait=true'],ok=False)
        api.call(['delete','configmap',name,'-n',system,'--ignore-not-found'],ok=False)


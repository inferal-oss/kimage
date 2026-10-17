#!/usr/bin/env python3
"""Populated future API data for prepared-reader rollback acceptance only."""
import copy
import hashlib
import json


def require(value,message):
    if not value:raise RuntimeError(message)

class FutureData:
    def __init__(self,api,system,architectures):
        self.api=api;self.system=system;self.architectures=set(architectures)
        self.report=dict(schema=1,test='populated-future-reader',baseline='0.6.1',future='0.6.2-argocd-test.0',sourceSchema=6,retainedSchema=7,complete=False)
    def schemas(self):
        return {name:self.api.get('customresourcedefinitions.apiextensions.k8s.io',name) for name in ['images.kimage.dev','imagereleases.kimage.dev','imagedemands.kimage.dev']}
    def write(self):
        old=self.api.prefix;self.api.prefix=old+'-future'
        try:self.api.case(None,spec_extensions=dict(futureNote='retained-future-data'),capabilities=['future-notes-v1'])
        finally:self.api.prefix=old
        self.case=self.api.cases[-1];self.image=self.api.get('images.kimage.dev',self.case['name'],self.api.namespace)
        self.owned={self.image['metadata']['uid']}|{i['metadata']['uid'] for i in self.api.get('images.kimage.dev',namespace=self.api.namespace,selector='kimage.dev/platform-parent='+self.image['metadata']['uid'])['items']}
        releases=self.api.get('imagereleases.kimage.dev',namespace=self.api.namespace)['items']
        self.releases={r['metadata']['name']:copy.deepcopy(r) for r in releases if r['spec']['imageUid'] in self.owned}
        native=[r for r in self.releases.values() if r['spec']['platform']['architecture']!='auto']
        require(native and {r['spec']['platform']['architecture'] for r in native}==self.architectures,'Future writer did not publish every native platform')
        require(all(r['spec']['provenance']['futureReceipt']=='future-notes-v1' for r in native),'Future provenance field was not populated')
        self.original_schemas=self.schemas();require(all(s['metadata']['annotations']['kimage.dev/schema-revision']=='7' for s in self.original_schemas.values()),'Future schema revision 7 was not installed')
        self.report.update(imageUid=self.image['metadata']['uid'],nativePlatforms=sorted(self.architectures),populatedImageField=True,populatedNativeProvenance=True,nativeReleaseCount=len(native))
    def after_return(self):
        def fenced():
            image=self.api.get('images.kimage.dev',self.case['name'],self.api.namespace)
            return any(c['type']=='BuildSupported' and c['status']=='False' for c in image.get('status',{}).get('conditions',[]))
        self.api.wait(fenced,'unchanged baseline reader fences populated future Image')
        current=self.api.get('images.kimage.dev',self.case['name'],self.api.namespace)
        require(current['metadata']['uid']==self.image['metadata']['uid'] and current['spec']==self.image['spec'],'Rollback changed or erased opaque Image fields')
        require(current['status']['releaseRef']==self.image['status']['releaseRef'] and any(c['type']=='Ready' and c['status']=='True' for c in current['status']['conditions']),'Unsupported build lost published readiness')
        for name,old in self.releases.items():
            release=self.api.get('imagereleases.kimage.dev',name,self.api.namespace)
            require(release['metadata']['uid']==old['metadata']['uid'] and release['spec']==old['spec'],'Rollback changed native release identity or opaque provenance')
        schemas=self.schemas()
        for name,old in self.original_schemas.items():
            new=schemas[name];require(new['metadata']['uid']==old['metadata']['uid'] and new['spec']==old['spec'] and new['metadata']['annotations']['kimage.dev/schema-revision']=='7','Binary rollback replaced the newer shared CRD')
        diagnostic=self.api.get('configmap','kimage-schema-status',self.system)['data']
        require(diagnostic['revision']=='7' and diagnostic['readerRevision']=='6' and diagnostic['readerVersion']=='0.6.1','Schema diagnostics conflate shared revision with baseline reader')
        # A newly declared unsupported Image has no existing status. Its first
        # fence must satisfy the unchanged required observedGeneration field.
        fresh=copy.deepcopy(self.image)
        fresh['metadata']=dict(name=self.case['name']+'-paused',annotations={'kimage.dev/required-capabilities':'["future-notes-v1"]'})
        fresh.pop('status',None)
        fresh=self.api.create(fresh)
        def fresh_paused():
            value=self.api.get('images.kimage.dev',fresh['metadata']['name'],self.api.namespace)
            return value if any(c['type']=='BuildSupported' and c['status']=='False' for c in value.get('status',{}).get('conditions',[])) else None
        paused=self.api.wait(fresh_paused,'fresh unsupported Image receives a valid paused status')
        require(paused['status']['observedGeneration']==paused['metadata']['generation'] and not paused['status'].get('releaseRef'),'Fresh unsupported Image was executed or lost its status')
        require(not self.api.get('images.kimage.dev',namespace=self.api.namespace,selector='kimage.dev/platform-parent='+fresh['metadata']['uid'])['items'],'Unsupported Image started native builds')
        self.report['freshUnsupportedImagePausedBeforeBuild']=True
        old=self.api.prefix;self.api.prefix=old+'-after-future'
        try:self.api.case(None)
        finally:self.api.prefix=old
        self.report.update(opaqueImageSpecPreserved=True,opaqueReleaseSpecsPreserved=True,publishedReadinessPreserved=True,unsupportedBuildPaused=True,unrelatedNativeBuildsAfterRollback=True,sharedCrdIdentityAndDefinitionsPreserved=True,schemaDiagnostic=dict(sharedRevision=7,readerRevision=6,readerVersion='0.6.1'),schemas=[dict(name=name,definitionSha256=hashlib.sha256(json.dumps(v['spec'],sort_keys=True,separators=(',',':')).encode()).hexdigest()) for name,v in sorted(schemas.items())],complete=True)
        return self.report

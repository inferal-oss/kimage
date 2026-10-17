#!/usr/bin/env python3
"""Opt-in native frozen OCI input acceptance against an existing installation.

Creates only named fixture Images, input ConfigMaps and consumer Deployments.
Consumer cleanup is UID-guarded. Registry data and releases remain retained.
"""
import argparse
import base64
import copy
import importlib.util
import json
from pathlib import Path
import subprocess
import uuid

ROOT=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location('mixed',ROOT/'scripts/e2e-mixed-cluster.py')
mixed=importlib.util.module_from_spec(spec);spec.loader.exec_module(mixed)
require=mixed.require
RECIPE='FROM toolchain AS build\nCOPY message /src/message\nRUN ["/tool", "transform", "/src/message", "/out/message"]\nFROM runtime\nCOPY --from=build /out/message /message\nENTRYPOINT ["/tool", "serve", "/message"]\n'


def build_probes(directory,architectures):
    directory.mkdir(parents=True,exist_ok=True)
    sysroot=Path(subprocess.check_output(['rustc','--print','sysroot'],text=True).strip())
    host=next(s.split(': ',1)[1] for s in subprocess.check_output(['rustc','-vV'],text=True).splitlines() if s.startswith('host: '))
    linker=sysroot/'lib/rustlib'/host/'bin/rust-lld'
    for arch in sorted(architectures):
        target={'amd64':'x86_64-unknown-linux-musl','arm64':'aarch64-unknown-linux-musl'}[arch]
        subprocess.run(['rustc','--edition=2024','--target',target,'-C','linker='+str(linker),'-C','link-self-contained=yes','-C','opt-level=z','-C','lto=yes','-C','strip=symbols','-C','panic=abort',str(ROOT/'scripts/fixtures/image-input-tool.rs'),'-o',str(directory/('tool-'+arch))],check=True)
        mixed.verify_probe(directory/('tool-'+arch),arch)


class ImageInputs:
    def __init__(self,api,probes,output,architectures=None):
        self.api=api;self.output=Path(output);self.output.mkdir(parents=True,exist_ok=False)
        record=json.loads(api.get('configmap',api.config['installation']+'-installation',api.system)['data']['installation.json'])
        require(record['release']=='0.6.0','Frozen input acceptance requires 0.6.0')
        self.architectures=set(architectures or record['options']['workerNodes'])
        require(self.architectures and self.architectures<={'amd64','arm64'},'Native worker architectures required')
        self.nodes={n['metadata']['name']:n for n in api.get('nodes')['items']}
        self.prefix='oci-input-'+uuid.uuid4().hex[:10]
        self.base=self.prefix+'-base';self.context=self.prefix+'-context'
        self.probes=Path(probes);self.consumers=[]
        self.report=dict(schema=1,test='frozen-oci-image-inputs',release='0.6.0',architectures=sorted(self.architectures),cases=[],checks=[],complete=False,cleanupComplete=False)

    def ready(self,name):
        image=self.api.get('images.kimage.dev',name,self.api.namespace)
        return image if image and mixed.is_ready(image) else None

    def base_image(self,name,version='1.0.0',architectures=None,replacement=False):
        image=mixed.image_body(name,self.prefix+'-tools')
        image['spec']['version']=version
        image['spec']['platforms']=[dict(os='linux',architecture=a) for a in sorted(architectures or self.architectures)]
        image['spec']['source']['build']['definition']['inline']='FROM scratch\nARG TARGETARCH\nCOPY tool-${TARGETARCH} /tool\nLABEL fixture='+('replacement' if replacement else version)+'\nENTRYPOINT ["/tool"]\n'
        return image

    def input_build(self,name,cache='Local',base=None):
        image=mixed.image_body(name,self.context)
        build=image['spec']['source']['build']
        build['definition']['inline']=RECIPE
        build['cache']=dict(mode=cache)
        build['imageInputs']=[dict(name='toolchain',image='platform://'+self.api.namespace+'/'+(base or self.base)+':1.0.0'),dict(name='runtime',image='platform://'+self.api.namespace+'/'+(base or self.base)+':2.0.0')]
        return image

    def consume(self,image,architecture=None):
        expected={architecture} if architecture else self.architectures
        name=self.prefix+'-consumer-'+uuid.uuid4().hex[:8]
        body=mixed.deployment_body(name,self.api.namespace,architecture)
        body['spec']['replicas']=len(expected)
        body['spec']['template']['spec']['containers'][0]['image']='platform://'+self.api.namespace+'/'+image+':1.0.0'
        resource=self.api.create(body);self.consumers.append((name,resource['metadata']['uid']))
        def running():
            pods=self.api.get('pods',namespace=self.api.namespace,selector='kimage.dev/mixed-e2e-case='+name)['items']
            if len(pods)!=len(expected) or not all(any(c['type']=='Ready' and c['status']=='True' for c in p.get('status',{}).get('conditions',[])) for p in pods):return None
            seen=set()
            for pod in pods:
                arch=self.nodes[pod['spec']['nodeName']]['status']['nodeInfo']['architecture'];seen.add(arch)
                logs=self.api.call(['logs',pod['metadata']['name'],'-n',self.api.namespace]).stdout
                require(('x86_64' if arch=='amd64' else 'aarch64')+': FROZEN OCI INPUTS WORK' in logs,'Native base/toolchain output mismatch')
                require('@sha256:' in pod['spec']['containers'][0]['image'],'Consumer was not digest pinned')
            require(seen==expected,'Unexpected native consumer placement')
            return pods
        self.api.wait(running,'native OCI input consumer '+str(sorted(expected)))

    def verify_materials(self,image,expected,original_uid):
        parent=self.api.get('images.kimage.dev',image,self.api.namespace)
        children=self.api.get('images.kimage.dev',namespace=self.api.namespace,selector='kimage.dev/platform-parent='+parent['metadata']['uid'])['items']
        require({c['spec']['platform']['architecture'] for c in children}==expected,'Unrequested native image input variant')
        materials=[]
        for child in children:
            self.api.wait(lambda:self.ready(child['metadata']['name']),'native input child ready')
            child=self.api.get('images.kimage.dev',child['metadata']['name'],self.api.namespace)
            release=self.api.get('imagereleases.kimage.dev',child['status']['releaseRef'],self.api.namespace)
            entries=release['spec']['provenance']['imageInputs']
            require([e['name'] for e in entries]==['toolchain','runtime'],'Missing image material provenance')
            require(all(e['imageUid']==original_uid and e['platform']==child['spec']['platform'] and e['manifest']['mediaType']=='application/vnd.oci.image.manifest.v1+json' for e in entries),'Frozen identity or native material mismatch')
            execution=release['spec']['provenance']['execution']
            materials.append(dict(platform=child['spec']['platform'],inputs=entries,execution=execution))
        return dict(image=image,imageUid=parent['metadata']['uid'],platforms=sorted(expected),materials=materials)

    def case(self,label,architecture=None,cache='Local'):
        name=self.prefix+'-'+label
        self.api.create(self.input_build(name,cache))
        self.consume(name,architecture)
        self.api.wait(lambda:self.ready(name),'frozen input Image ready')
        receipt=self.verify_materials(name,{architecture} if architecture else self.architectures,self.original_uid)
        receipt['case']=label
        if cache=='Disabled':require(all(m['execution']['cacheMode']=='Disabled' and m['execution']['cachedExecutionSteps']==0 and m['execution']['executedSteps']>=1 for m in receipt['materials']),'Disabled cache reused execution')
        if label=='warm':require(all(m['execution']['cachedExecutionSteps']>=1 for m in receipt['materials']),'Warm solve did not reuse toolchain execution')
        self.report['cases'].append(receipt)
        print('PASS: frozen OCI inputs '+label+' on '+','.join(receipt['platforms']),flush=True)
        return name

    def denied(self,label,body,reason,architecture=None):
        name=self.prefix+'-'+label;body['metadata']['name']=name
        body['spec'].pop('platforms',None);body['spec']['platform']=dict(os='linux',architecture=architecture or next(iter(sorted(self.architectures))))
        self.api.create(body)
        def failed():
            image=self.api.get('images.kimage.dev',name,self.api.namespace)
            conditions=image.get('status',{}).get('conditions',[])
            return image if any(c['type']=='Ready' and c['status']=='False' and c['reason']==reason for c in conditions) else None
        image=self.api.wait(failed,'refused '+label)
        require(not image.get('status',{}).get('releaseRef'),'Denied solve published a release')
        require(not (image.get('status',{}).get('build') or {}).get('execution'),'Denied attempt reported successful execution')
        self.report['checks'].append(label+': '+reason+' without publication')

    def run(self):
        try:
            data={'tool-'+a:base64.b64encode(mixed.verify_probe(self.probes/('tool-'+a),a)).decode() for a in sorted(self.architectures)}
            require(sum(map(len,data.values()))<=1_200_000,'Fixture tools exceed ConfigMap limit')
            self.api.create(dict(apiVersion='v1',kind='ConfigMap',metadata=dict(name=self.prefix+'-tools'),immutable=True,binaryData=data))
            self.api.create(dict(apiVersion='v1',kind='ConfigMap',metadata=dict(name=self.context),immutable=True,data={'message':'frozen OCI inputs work\n'}))
            base=self.api.create(self.base_image(self.base));self.original_uid=base['metadata']['uid']
            self.api.wait(lambda:self.ready(self.base),'published native toolchain base')
            # A different exact release supplies runtime bytes, with the same native tools.
            base=self.api.get('images.kimage.dev',self.base,self.api.namespace)
            spec=self.base_image(self.base,'2.0.0')['spec']
            self.api.call(['patch','images.kimage.dev',self.base,'-n',self.api.namespace,'--type=merge','--patch',json.dumps(dict(metadata=dict(uid=base['metadata']['uid'],resourceVersion=base['metadata']['resourceVersion']),spec=spec))])
            self.api.wait(lambda:self.ready(self.base),'published native runtime base')
            self.case('unknown')
            for arch in sorted(self.architectures):self.case(arch,arch)
            arch=next(iter(sorted(self.architectures)))
            self.case('warm',arch)
            self.case('disabled',arch,'Disabled')
            body=self.input_build('unused');body['spec']['source']['build']['definition']['inline']='FROM undeclared.invalid/base:latest\n'
            self.denied('undeclared',body,'ContainerfileSolveFailed')
            missing=self.input_build('unused');missing['spec']['source']['build']['imageInputs'][0]['image']='platform://'+self.api.namespace+'/not-published-'+uuid.uuid4().hex[:8]+':1.0.0'
            self.denied('missing',missing,'ImageInputNotPublished')
            if self.architectures=={'amd64','arm64'}:
                single=self.prefix+'-amd-base'
                self.api.create(self.base_image(single,architectures={'amd64'}))
                self.api.wait(lambda:self.ready(single),'single-platform base publication')
                wrong=self.input_build('unused',base=single)
                for entry in wrong['spec']['source']['build']['imageInputs']:entry['image']=entry['image'].replace(':2.0.0',':1.0.0')
                self.denied('missing-platform',wrong,'ImageInputPlatformUnavailable',architecture='arm64')
                name=self.prefix+'-accumulation';self.api.create(self.input_build(name));self.consume(name,'amd64')
                self.api.wait(lambda:self.ready(name),'first native platform before dependency replacement')
                self.verify_materials(name,{'amd64'},self.original_uid)
                base=self.api.get('images.kimage.dev',self.base,self.api.namespace)
                path='/apis/kimage.dev/v1alpha1/namespaces/'+self.api.namespace+'/images/'+self.base
                self.api.call(['delete','--raw',path,'-f','-'],dict(apiVersion='v1',kind='DeleteOptions',preconditions=dict(uid=base['metadata']['uid'])))
                self.api.wait(lambda:self.api.get('images.kimage.dev',self.base,self.api.namespace) is None,'fixture dependency deletion')
                replacement=self.api.create(self.base_image(self.base,replacement=True))
                require(replacement['metadata']['uid']!=self.original_uid,'Dependency identity did not change')
                self.api.wait(lambda:self.ready(self.base),'replacement base publication')
                self.consume(name,'arm64');self.api.wait(lambda:self.ready(name) and len(self.ready(name)['status'].get('platformReleases',{}))==2,'later arm64 from original frozen root')
                receipt=self.verify_materials(name,self.architectures,self.original_uid);receipt['case']='later-platform-after-base-recreation';self.report['cases'].append(receipt)
                self.report['checks'].append('Later arm64 demand keeps the original base UID and complete OCI root after same-name base replacement')
            self.report['complete']=True
        finally:
            errors=[]
            for name,uid in self.consumers:
                try:
                    current=self.api.get('deployment',name,self.api.namespace)
                    if current:
                        require(current['metadata']['uid']==uid,'Consumer cleanup identity changed')
                        self.api.call(['delete','--raw','/apis/apps/v1/namespaces/'+self.api.namespace+'/deployments/'+name,'-f','-'],dict(apiVersion='v1',kind='DeleteOptions',preconditions=dict(uid=uid),propagationPolicy='Foreground'))
                        self.api.wait(lambda:self.api.get('deployment',name,self.api.namespace) is None,'owned consumer cleanup')
                except Exception:
                    errors.append(dict(consumer=name,reason='OwnedConsumerCleanupFailed'))
            self.report['cleanupComplete']=not errors
            self.report['cleanupErrors']=errors
            (self.output/'results.json').write_text(json.dumps(self.report,indent=2)+'\n')
            require(not errors,'Owned consumer cleanup incomplete; see bounded receipt')
        return self.report


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--run',action='store_true');parser.add_argument('--config',type=Path)
    parser.add_argument('--build-probes',type=Path);parser.add_argument('--architectures',nargs='+',choices=['amd64','arm64'],default=['amd64','arm64'])
    args=parser.parse_args()
    if args.build_probes:build_probes(args.build_probes,set(args.architectures));return
    require(args.run and args.config,'Use --run --config for live acceptance')
    config=json.loads(args.config.read_text());api=mixed.MixedCluster(config)
    ImageInputs(api,config['probes'],api.output/'image-inputs').run()

if __name__=='__main__':main()

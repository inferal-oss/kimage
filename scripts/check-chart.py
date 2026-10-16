#!/usr/bin/env python3
"""Lint Helm and compare its complete resources with the native install planner."""
import argparse
import copy
import json
import os
from pathlib import Path
import subprocess
import shutil
import tempfile

ROOT = Path(__file__).resolve().parents[1]
ROLES = ['admission', 'builder', 'exporter', 'gateway', 'input', 'operator', 'proxy', 'registry']


def node(name, arch):
    return dict(apiVersion='v1', kind='Node', metadata=dict(name=name, uid='uid-'+name,
        labels={'kubernetes.io/os':'linux', 'kubernetes.io/arch':arch}), spec={}, status=dict(
        conditions=[dict(type='Ready',status='True')], nodeInfo=dict(architecture=arch,
        operatingSystem='linux',kubeletVersion='v1.36.5+rke2r1',containerRuntimeVersion='containerd://2.3.4-k3s1',
        bootID='boot',kernelVersion='fixture',machineID='machine',osImage='fixture',systemUUID='system')))


def normalize(value):
    if isinstance(value, list):
        return [normalize(v) for v in value]
    if not isinstance(value, dict):
        return value
    value = copy.deepcopy(value)
    if value.get('kind') == 'Secret':
        value['data'] = {key:'<private>' for key in value['data']}
    metadata = value.get('metadata', {})
    if 'kimage.dev/manager' in metadata.get('annotations', {}):
        metadata['annotations']['kimage.dev/manager'] = '<manager>'
    if 'kimage.dev/scope' in metadata.get('annotations', {}):
        metadata['annotations']['kimage.dev/scope'] = json.loads(metadata['annotations']['kimage.dev/scope'])
    if value.get('kind') == 'ConfigMap':
        for key, content in list(value.get('data', {}).items()):
            if key.endswith('.json'):
                value['data'][key] = json.loads(content)
    if 'caBundle' in value:
        value['caBundle'] = '<private>'
    return {key:normalize(v) for key,v in value.items()}


def index(resources):
    return {(v['apiVersion'],v['kind'],v['metadata'].get('namespace'),v['metadata']['name']):normalize(v) for v in resources}


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--helm',default=os.environ.get('KIMAGE_HELM_BINARY','helm'))
    args=parser.parse_args()
    helm=str(Path(args.helm).resolve()) if '/' in args.helm else args.helm
    subprocess.run(['python3',str(ROOT/'scripts/sync-chart-resources.py'),'--check'],check=True)
    subprocess.run(['cargo','build','--locked','--quiet','-p','kimage-control','--example','pull-plan'],cwd=ROOT,check=True)
    metadata=json.loads(subprocess.check_output(['cargo','metadata','--locked','--no-deps','--format-version','1'],cwd=ROOT,text=True))
    version=next(p['version'] for p in metadata['packages'] if p['name']=='kimage-control')
    major,minor,_=version.split('.')
    future=f'{major}.{int(minor)+1}.0'
    example=Path(metadata['target_directory'])/'debug/examples/pull-plan'
    with tempfile.TemporaryDirectory(prefix='kimage-chart-') as temporary:
        work=Path(temporary)
        digests={role:'sha256:'+'a'*64 for role in ROLES}
        catalog=dict(schema=1,release=version,apiSchemaRevision=5,repository='ghcr.io/inferal-oss/kimage',components={role:dict(
            version='0.3.0' if role in ['gateway','proxy'] else '0.33.0' if role=='builder' else '3.1.2' if role=='registry' else version,
            digest=digests[role],platforms=[dict(os='linux',architecture=arch) for arch in ['amd64','arm64']]) for role in ROLES})
        (work/'catalog.json').write_text(json.dumps(catalog))
        for case,(scope,architectures,existing,release) in enumerate([
            ('namespace',['amd64','arm64'],False,version),('cluster',['amd64','arm64'],False,version),
            ('namespace',['arm64'],True,version),('namespace',['amd64'],False,version),
            ('namespace',['amd64','arm64'],False,future)]):
            selected=copy.deepcopy(catalog);selected['release']=release
            for role in ['admission','exporter','input','operator']:selected['components'][role]['version']=release
            (work/'catalog.json').write_text(json.dumps(selected))
            chart=ROOT/'charts/kimage'
            if release!=version:
                chart=work/'future-chart';shutil.copytree(ROOT/'charts/kimage',chart)
                path=chart/'Chart.yaml';path.write_text(path.read_text().replace(version,release))
            nodes=[node('node-'+arch,arch) for arch in architectures]
            options=dict(namespace='kimage-system',installation='kimage',scope=scope,
                applicationNamespace='apps' if scope=='namespace' else None,excludedNamespaces=['excluded'] if scope=='cluster' else [],
                runtimeProfile='inferal',registryNode=None,exportOrigins=['https://registry.example/'] if case<2 else [],storageClass=None,storageSize='1Gi',existingClaim='retained' if existing else None,port=17701)
            values=dict(scope=scope,applicationNamespace='apps' if scope=='namespace' else '',excludedNamespaces=options['excludedNamespaces'],
                nodes=nodes,exporter=dict(approvedOrigins=options['exportOrigins']),images=dict(digests=digests),persistence=dict(existingClaim='retained' if existing else ''))
            for name,value in [('options',options),('nodes',nodes),('values',values)]:
                (work/(name+'.json')).write_text(json.dumps(value))
            native=work/f'native-{case}.json'
            subprocess.run([str(example),work/'options.json',work/'catalog.json',work/'nodes.json',native],check=True)
            command=[helm,'template','kimage',str(chart),'--namespace','kimage-system','-f',str(work/'values.json')]
            result=subprocess.run(command,text=True,capture_output=True)
            if result.returncode:raise RuntimeError(result.stderr)
            rendered=[]
            for document in result.stdout.split('\n---'):
                lines=[line for line in document.splitlines() if line.strip() and not line.startswith('#') and line.strip()!='---']
                if lines:rendered.append(json.loads('\n'.join(lines)))
            expected=index(json.loads(native.read_text()));actual=index(rendered)
            assert actual.keys()==expected.keys(),f'{scope}/{architectures}: resource identity mismatch: {actual.keys() ^ expected.keys()}'
            for key in expected:
                assert actual[key]==expected[key],f'{scope}/{architectures}: Helm/native mismatch in {key}'
            subprocess.run([helm,'lint',str(chart),'--namespace','kimage-system','-f',str(work/'values.json')],check=True,stdout=subprocess.DEVNULL)
            print(f'{release}, {scope}, {"+".join(architectures)}: {len(actual)} matching resources, retained storage and pinned images')
        # Invalid administrator inputs must fail during rendering, before any API writes.
        for changes,reason in [
            (dict(scope='cluster',applicationNamespace='apps'),'applicationNamespace'),
            (dict(credentials=dict(registry={'ca.key':'private-ca-must-not-be-deployed'})),'InvalidInstallationCredentialKeys'),
            (dict(exporter=dict(approvedOrigins=['http://registry.example/'])),'InvalidApprovedExportOrigin'),
            (dict(images=dict(digests=dict(digests,operator='latest'))),'MissingComponentDigest')]:
            bad=copy.deepcopy(values);bad.update(changes)
            (work/'bad.json').write_text(json.dumps(bad))
            result=subprocess.run([helm,'template','kimage',str(ROOT/'charts/kimage'),'--namespace','kimage-system','-f',str(work/'bad.json')],text=True,capture_output=True)
            assert result.returncode and reason in result.stderr, reason+': invalid chart inputs were accepted'
        print('Helm/native manifests match for both scopes and native platforms.')


if __name__=='__main__':
    main()

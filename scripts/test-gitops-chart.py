#!/usr/bin/env python3
"""Prove Argo rendering is offline, stable and excludes private/runtime objects."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile

ROOT=Path(__file__).resolve().parents[1]
ROLES=['admission','builder','exporter','gateway','input','operator','proxy','registry']

def documents(text):
    return [json.loads('\n'.join(line for line in document.splitlines() if line.strip() and not line.startswith('#') and line.strip()!='---')) for document in text.split('\n---') if any(line.strip() and not line.startswith('#') and line.strip()!='---' for line in document.splitlines())]

def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--helm',default='helm');a=p.parse_args()
    with tempfile.TemporaryDirectory() as directory:
        work=Path(directory)
        env=dict(os.environ,KUBECONFIG=str(work/'no-cluster-access'))
        for scope in ['namespace','cluster']:
            values=dict(gitops=dict(enabled=True),scope=scope,applicationNamespace='apps' if scope=='namespace' else '',images=dict(digests={r:'sha256:'+str(i+1)*64 for i,r in enumerate(ROLES)}))
            path=work/'values.json';path.write_text(json.dumps(values))
            command=[a.helm,'template','kimage',str(ROOT/'charts/kimage'),'--namespace','kimage-system','-f',str(path)]
            first=subprocess.check_output(command,env=env,text=True)
            second=subprocess.check_output(command,env=env,text=True)
            assert first==second,'rendered desired state changed without a configuration change'
            objects=documents(first)
            assert sorted(o['kind'] for o in objects)==['ClusterRole','ClusterRoleBinding','ConfigMap','Deployment','ServiceAccount']
            assert all('kimage.dev/installation' not in o['metadata'].get('labels',{}) for o in objects),'runtime uninstall must not delete the active provisioner'
            role=next(o for o in objects if o['kind']=='ClusterRole')
            assert all('pods/portforward' not in rule['resources'] for rule in role['rules']), 'in-cluster transitions verify retained graphs directly without API port-forward privileges'
            desired=json.loads(next(o for o in objects if o['kind']=='ConfigMap')['data']['desired.json'])
            assert desired['options']['scope']==scope and not desired['options']['workerNodes']
            assert desired['credentialSecrets']=={} and desired['buildPolicy'] is None and desired['nodePolicy'] is None
            deployment=next(o for o in objects if o['kind']=='Deployment')
            pod=deployment['spec']['template']['spec']
            assert deployment['spec']['strategy']['type']=='Recreate'
            assert pod['containers'][0]['image'].endswith('@'+values['images']['digests']['operator'])
            assert pod['containers'][0]['readinessProbe']['httpGet']['path']=='/readyz'
            assert 'ignoreDifferences' not in first and 'tls.key' not in first and 'ca.key' not in first
            for change in [dict(nodes=[{'metadata':{'name':'live-node'}}]),dict(credentials=dict(registry={'tls.key':'private'})),dict(gitops=dict(enabled=True,targetCatalog=dict(release='0.5.0')))]:
                bad=dict(values,**change);path.write_text(json.dumps(bad))
                assert subprocess.run(command,env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL).returncode,'unsupported Argo rendering input was accepted'
            # --reuse-values from 0.5.0 has no gitops object at all.
            if scope=='namespace':
                chart=work/'legacy-values-chart';import shutil
                shutil.copytree(ROOT/'charts/kimage',chart)
                source=(chart/'values.yaml').read_text();source=source[:source.index('gitops:')]
                (chart/'values.yaml').write_text(source)
                legacy=dict(values);legacy.pop('gitops');path.write_text(json.dumps(legacy))
                check=subprocess.run([a.helm,'template','kimage',str(chart),'--namespace','kimage-system','-f',str(path)],env=env,text=True,capture_output=True)
                assert 'nil pointer' not in check.stderr, 'pre-Argo Helm values must remain renderable'
            print('PASS: '+scope+' Argo CD chart renders deterministically without Kubernetes, Nodes or credentials')

if __name__=='__main__':main()

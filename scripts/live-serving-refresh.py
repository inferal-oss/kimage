#!/usr/bin/env python3
"""Apply a compatible serving correction, preserving retained fixture identities."""
import argparse
import hashlib
import json
import os
import pathlib
import subprocess

p=argparse.ArgumentParser(description=__doc__)
for key in ['kubeconfig','options','previous-bundle','bundle','installer','output']:
    p.add_argument('--'+key,type=pathlib.Path,required=True)
a=p.parse_args()
assert 'server: https://127.0.0.1:27643' in a.kubeconfig.read_text()
o=json.loads(a.options.read_text());assert o['installation']=='kimage-clean'
a.output.mkdir(parents=True,exist_ok=True)
env=dict(os.environ,KUBECONFIG=str(a.kubeconfig.resolve()))
b=['kubectl','--kubeconfig',str(a.kubeconfig),'--request-timeout=45s']


def trusted(path):return 'sha256:'+hashlib.sha256((path/'inventory.json').read_bytes()).hexdigest()


def objects(kind,ns):
    values=json.loads(subprocess.check_output(b+['-n',ns,'get',kind,'-o','json'],timeout=60))['items']
    return {v['metadata']['name']:dict(uid=v['metadata']['uid'],spec=v['spec']) for v in values}


baseline_path=a.output/'baseline.json'
if baseline_path.exists():baseline=json.loads(baseline_path.read_text())
else:
    baseline=dict(previousInventory=trusted(a.previous_bundle),targetInventory=trusted(a.bundle),pvc=objects('pvc','kimage-system'),releases=objects('imagereleases','kimage-app'),schemas=objects('crd','kimage-system'))
    baseline_path.write_text(json.dumps(baseline,indent=2)+'\n')
assert baseline['previousInventory']==trusted(a.previous_bundle) and baseline['targetInventory']==trusted(a.bundle)
args=[str(a.installer.resolve()),'upgrade-serving']
for key,value in o.items():args+=['--'+''.join('-'+c.lower() if c.isupper() else c for c in key),str(value)]
args+=['--bundle',str(a.bundle.resolve()),'--trusted-inventory',trusted(a.bundle),'--previous-bundle',str(a.previous_bundle.resolve()),'--previous-trusted-inventory',trusted(a.previous_bundle)]
with (a.output/'upgrade.log').open('wb') as log:
    value=subprocess.run(args,env=env,stdout=log,stderr=subprocess.STDOUT,timeout=1200)
assert value.returncode==0,'native correction failed; inspect private upgrade log'
assert objects('pvc','kimage-system')==baseline['pvc']
assert objects('imagereleases','kimage-app')==baseline['releases']
schemas=objects('crd','kimage-system')
assert all(schemas[name]==baseline['schemas'][name] for name in ['images.kimage.dev','imagereleases.kimage.dev'])
target=json.loads((a.bundle/'inventory.json').read_text())
pods=json.loads(subprocess.check_output(b+['-n','kimage-system','get','pods','-l','kimage.dev/installation=kimage-clean','-o','json'],timeout=60))['items']
serving=[v for v in pods if v['metadata']['name'] in ['kimage-clean-registry','kimage-clean-proxy'] or (v['metadata']['name'].startswith('kimage-clean-n-') and v['metadata']['name'].endswith('-proxy'))]
assert len(serving)==3
for pod in serving:
    assert any(c['type']=='Ready' and c['status']=='True' for c in pod['status']['conditions'])
    for c in pod['spec']['containers']:assert c['image'].endswith('@'+target['components'][c['name']]['root']['digest'])
previous=json.loads((a.previous_bundle/'inventory.json').read_text())
changed=[role for role in previous['components'] if previous['components'][role]!=target['components'][role]]
result=dict(previousInventory=trusted(a.previous_bundle),targetInventory=trusted(a.bundle),changedComponents=changed,nativeCompatibleServingRollout=True,allServingPodsReady=True,pvcReleaseCrdIdentitiesSpecsPreserved=True)
(a.output/'evidence.json').write_text(json.dumps(result,indent=2)+'\n')
print('native serving correction passed with retained identities and readiness preserved',flush=True)

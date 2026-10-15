#!/usr/bin/env python3
"""Use an uncached retained release after native two-node bundle recovery."""
import argparse
import json
import pathlib
import subprocess
import time

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--kubeconfig', type=pathlib.Path, required=True)
p.add_argument('--case', required=True)
p.add_argument('--output', type=pathlib.Path, required=True)
a = p.parse_args()
assert 'server: https://127.0.0.1:27643' in a.kubeconfig.read_text()
assert a.case.startswith('kimage-coexist-') and a.case.replace('-', '').isalnum()
b = ['kubectl', '--kubeconfig', str(a.kubeconfig), '--request-timeout=45s', '-n', 'kimage-app']


def call(args, body=None):
    value = subprocess.run(b + args, input=None if body is None else json.dumps(body).encode(), capture_output=True, timeout=100)
    if value.returncode:
        raise RuntimeError(value.stderr.decode()[:1800])
    return value.stdout


def get(kind, name):
    return json.loads(call(['get', kind, name, '-o', 'json']))


image = get('image', a.case)
release = get('imagerelease', image['status']['releaseRef'])
root = release['spec']['root']['digest']
cache = json.loads(subprocess.check_output(['python3', 'scripts/isolated-fixture.py', 'request', '--directory', '.local/qemu-second', '--operation', 'images'], timeout=130))
assert not any(root in ref for item in cache['images'] for ref in item.get('repoDigests', []))
call(['delete', 'image', a.case, '--wait=true', '--timeout=120s'])
call(['delete', 'configmap', a.case + '-context', '--wait=true', '--timeout=30s'])
assert get('imagerelease', release['metadata']['name'])['spec'] == release['spec']
name = a.case + '-retained-recovery'
call(['create', '-f', '-'], dict(apiVersion='v1', kind='Pod', metadata=dict(name=name), spec=dict(nodeSelector={'kubernetes.io/hostname': 'kimage-clean-2'}, automountServiceAccountToken=False, containers=[dict(name='app', image='platform://kimage-app/' + a.case + ':1.0.0', imagePullPolicy='IfNotPresent')])))
end = time.monotonic() + 300
while time.monotonic() < end:
    pod = get('pod', name)
    if any(c['type'] == 'Ready' and c['status'] == 'True' for c in pod.get('status', {}).get('conditions', [])):
        break
    time.sleep(2)
else:
    raise RuntimeError('retained normal pull after bundle recovery deadline')
assert pod['spec']['containers'][0]['image'].endswith('@' + root) and pod['spec']['containers'][0]['imagePullPolicy'] == 'IfNotPresent'
assert get('imagerelease', release['metadata']['name'])['spec'] == release['spec']
a.output.write_text(json.dumps(dict(releaseUid=release['metadata']['uid'], root=release['spec']['root'], rootAbsentFromTargetCache=True, sourceImageAndConfigMapGone=True, retainedCatalogFallback=True, normalIfNotPresentPullAfterBundleRecovery=True, podUid=pod['metadata']['uid']), indent=2) + '\n')
print('uncached retained release pulled normally after bundle recovery and source/Image deletion', flush=True)

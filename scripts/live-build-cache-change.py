#!/usr/bin/env python3
"""Prove a changed ordinary RUN bind input invalidates real Local cache."""
import argparse
import copy
import hashlib
import json
import pathlib
import subprocess
import time

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--kubeconfig', type=pathlib.Path, required=True)
p.add_argument('--baseline-image', required=True)
p.add_argument('--case', required=True)
p.add_argument('--output', type=pathlib.Path, required=True)
a = p.parse_args()
assert 'server: https://127.0.0.1:27643' in a.kubeconfig.read_text()
assert a.baseline_image.startswith('kimage-matrix-') and a.case.startswith('kimage-cache-change-')
b = ['kubectl', '--kubeconfig', str(a.kubeconfig), '--request-timeout=45s', '-n', 'kimage-app']


def call(args, body=None):
    return subprocess.check_output(b + args, input=None if body is None else json.dumps(body).encode(), timeout=65)


def get(kind, name):
    return json.loads(call(['get', kind, name, '-o', 'json']))


baseline = get('image', a.baseline_image)
old = get('imagerelease', baseline['status']['releaseRef'])
assert old['status']['verified']
build = copy.deepcopy(baseline['spec']['source']['build'])
build['cache'] = dict(mode='Local')
context = next(v for v in build['inputs']['volumes'] if v['name'] == 'context')['configMap']
original = get('configmap', context['name'])
changed = dict(apiVersion='v1', kind='ConfigMap', metadata=dict(name=a.case + '-context'),
               data=dict(original.get('data', {})), binaryData=dict(original.get('binaryData', {})))
assert len(changed['data']['message']) == 32
changed['data']['message'] = hashlib.sha256((changed['data']['message'] + '/changed').encode()).hexdigest()[:32]
call(['create', '-f', '-'], changed)
context['name'] = changed['metadata']['name']
call(['create', '-f', '-'], dict(apiVersion='kimage.dev/v1alpha1', kind='Image', metadata=dict(name=a.case), spec=dict(
    version='1.0.0', platform=dict(os='linux', architecture='amd64'), source=dict(build=build))))
end = time.monotonic() + 900
while time.monotonic() < end:
    image = get('image', a.case)
    if any(c['type'] == 'Ready' and c['status'] == 'True' for c in image.get('status', {}).get('conditions', [])):
        break
    time.sleep(3)
else:
    raise RuntimeError('changed-input Local solve deadline')
release = get('imagerelease', image['status']['releaseRef'])
execution = release['spec']['provenance']['execution']
assert execution['cacheMode'] == 'Local'
assert execution['cachedExecutionSteps'] == 0 and execution['executedSteps'] >= 1
assert release['spec']['snapshot'] != old['spec']['snapshot']
result = dict(baselineReleaseUid=old['metadata']['uid'], releaseUid=release['metadata']['uid'],
    execution=execution, ordinarySnapshotChanged=True, localCacheInvalidatedForChangedBindInput=True)
a.output.parent.mkdir(parents=True, exist_ok=True)
a.output.write_text(json.dumps(result, indent=2) + '\n')
print('changed ordinary bind input executes RUN under real Local cache', flush=True)

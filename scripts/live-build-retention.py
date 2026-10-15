#!/usr/bin/env python3
"""Verify retained Direct artifacts survive worker/cache/input/Image removal."""
import argparse
import json
import pathlib
import subprocess
import time

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--kubeconfig', type=pathlib.Path, required=True)
p.add_argument('--matrix-case', required=True)
p.add_argument('--output', type=pathlib.Path, required=True)
a = p.parse_args()
assert 'server: https://127.0.0.1:27643' in a.kubeconfig.read_text()
assert a.matrix_case.startswith('kimage-matrix-') and a.matrix_case.replace('-', '').isalnum()
b = ['kubectl', '--kubeconfig', str(a.kubeconfig), '--request-timeout=45s']
ns = 'kimage-app'
name = a.matrix_case + '-pvc'


def call(args, namespace=ns):
    return subprocess.check_output(b + ['-n', namespace] + args, timeout=65)


def get(kind, name, namespace=ns):
    return json.loads(call(['get', kind, name, '-o', 'json'], namespace))


def is_ready(value):
    return any(c['type'] == 'Ready' and c['status'] == 'True' for c in value.get('status', {}).get('conditions', []))


end = time.monotonic() + 180
while time.monotonic() < end:
    image = get('image', name)
    if is_ready(image):
        break
    time.sleep(2)
else:
    raise RuntimeError('published PVC Image not ready before worker removal')
release = get('imagerelease', image['status']['releaseRef'])
assert release['status']['verified']
worker = get('pod', 'kimage-clean-builder', 'kimage-system')
call(['delete', 'pod', 'kimage-clean-builder', '--grace-period=1', '--wait=true', '--timeout=90s'], 'kimage-system')
samples = 0
started = time.monotonic()
end = started + 180
new_worker = None
while time.monotonic() < end:
    current = get('image', name)
    assert is_ready(current), 'worker/cache loss regressed retained artifact readiness'
    assert current['status']['releaseRef'] == release['metadata']['name']
    samples += 1
    raw = call(['get', 'pod', 'kimage-clean-builder', '--ignore-not-found', '-o', 'json'], 'kimage-system')
    if raw:
        candidate = json.loads(raw)
        if candidate['metadata']['uid'] != worker['metadata']['uid'] and is_ready(candidate):
            new_worker = candidate
    if time.monotonic() - started >= 45 and new_worker:
        break
    time.sleep(1)
assert new_worker, 'worker restoration deadline'
current_release = get('imagerelease', release['metadata']['name'])
assert current_release['metadata']['uid'] == release['metadata']['uid']
assert current_release['spec'] == release['spec']
call(['delete', 'image', name, '--wait=true', '--timeout=90s'])
call(['delete', 'pvc', a.matrix_case + '-source', '--wait=true', '--timeout=90s'])
for kind, suffix in [('configmap', '-context'), ('configmap', '-definition'), ('secret', '-private')]:
    call(['delete', kind, a.matrix_case + suffix, '--wait=true', '--timeout=60s'])
call(['delete', 'pod', 'kimage-clean-builder', '--grace-period=1', '--wait=true', '--timeout=90s'], 'kimage-system')
pod_name = a.matrix_case + '-retained'
pod = dict(apiVersion='v1', kind='Pod', metadata=dict(name=pod_name), spec=dict(
    restartPolicy='Never', automountServiceAccountToken=False, containers=[dict(name='app',
        image='platform://kimage-app/' + name + ':1.0.0', imagePullPolicy='IfNotPresent',
        resources=dict(requests=dict(cpu='5m', memory='8Mi'), limits=dict(cpu='100m', memory='32Mi')))]))
subprocess.run(b + ['-n', ns, 'create', '-f', '-'], input=json.dumps(pod).encode(), check=True, timeout=65)
end = time.monotonic() + 240
while time.monotonic() < end:
    current = get('pod', pod_name)
    if is_ready(current):
        break
    time.sleep(2)
else:
    raise RuntimeError('retained workload did not start')
assert current['spec']['containers'][0]['image'].endswith('@' + release['spec']['root']['digest'])
assert call(['logs', pod_name]).decode().strip() == 'kimage build fixture ready'
assert not call(['get', 'pod', 'kimage-clean-builder', '--ignore-not-found', '-o', 'name'], 'kimage-system')
logs = call(['logs', 'kimage-clean-proxy', '--tail=3000'], 'kimage-system')
assert release['spec']['root']['digest'].encode() in logs
result = dict(releaseUid=release['metadata']['uid'], root=release['spec']['root'],
    oldWorkerUid=worker['metadata']['uid'], restoredWorkerUid=new_worker['metadata']['uid'],
    readySamplesDuringCacheReplacement=samples, readinessPreserved=True, noRebuild=True,
    sourceObjectsAndPvcRemoved=True, imageRemoved=True, workerAndCacheAbsent=True,
    retainedWorkloadUid=current['metadata']['uid'], retainedWorkloadReady=True, runtimeRootRequestObserved=True)
a.output.parent.mkdir(parents=True, exist_ok=True)
a.output.write_text(json.dumps(result, indent=2) + '\n')
print('worker/cache eviction preserves readiness; retained artifact runs without inputs, Image or worker', flush=True)

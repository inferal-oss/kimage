#!/usr/bin/env python3
"""Exercise real in-flight Direct cancellation and operator restart fencing."""
import argparse
import base64
import json
import pathlib
import subprocess
import time

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--kubeconfig', type=pathlib.Path, required=True)
p.add_argument('--probe', type=pathlib.Path, required=True)
p.add_argument('--output', type=pathlib.Path, required=True)
p.add_argument('--case', required=True)
p.add_argument('--action', choices=['generation', 'input', 'delete', 'restart'], required=True)
a = p.parse_args()
assert 'server: https://127.0.0.1:27643' in a.kubeconfig.read_text()
assert a.case.startswith('kimage-lifecycle-') and a.case.replace('-', '').isalnum()
a.output.parent.mkdir(parents=True, exist_ok=True)
b = ['kubectl', '--kubeconfig', str(a.kubeconfig), '--request-timeout=45s']
ns = 'kimage-app'


def call(args, body=None, namespace=ns):
    r = subprocess.run(b + ['-n', namespace] + args, input=None if body is None else json.dumps(body).encode(), capture_output=True, timeout=65)
    if r.returncode:
        raise RuntimeError(r.stderr.decode()[:2000])
    return r.stdout


def get(kind, name, namespace=ns):
    return json.loads(call(['get', kind, name, '-o', 'json'], namespace=namespace))


def create(body, namespace=ns):
    call(['create', '-f', '-'], body, namespace)


def until(predicate, label, seconds=240):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        result = predicate()
        if result:
            return result
        time.sleep(2)
    raise RuntimeError(label + ' deadline')


source = a.case + '-input'
probe = a.probe.read_bytes()
assert len(probe) <= 512 * 1024
create(dict(apiVersion='v1', kind='ConfigMap', metadata=dict(name=source), binaryData=dict(probe=base64.b64encode(probe).decode())))
slow = 'FROM scratch\nCOPY probe /probe\nRUN ["/probe","wait","90"]\nENTRYPOINT ["/probe"]\n'
build = dict(format='Containerfile', execution=dict(strategy='Direct', builderRef='default'),
             definition=dict(inline=slow), cache=dict(mode='Disabled'), inputs=dict(contextPath='/workspace/context',
                 volumes=[dict(name='context', configMap=dict(name=source, defaultMode=365))],
                 volumeMounts=[dict(name='context', mountPath='/workspace/context', readOnly=True)]))
create(dict(apiVersion='kimage.dev/v1alpha1', kind='Image', metadata=dict(name=a.case), spec=dict(
    version='1.0.0', source=dict(build=build), platform=dict(os='linux', architecture='amd64'))))


def submitted():
    image = get('image', a.case)
    status = image.get('status', {})
    if not any(c.get('reason') == 'Executing' for c in status.get('conditions', [])):
        return None
    id_ = status['build']['snapshotId']
    client = 'build-' + id_[:20] + '-client'
    phase = json.loads(call(['exec', client, '-c', 'client', '--', '/snapshot/session', 'build-phase']))
    if phase['phase'] != 'Submitted':
        return None
    # Live in-use execution storage proves that the real engine started RUN.
    # Build history remains disabled; descriptions stay in process memory.
    args = ['/buildctl', '--addr', 'tcp://kimage-clean-builder.kimage-system.svc:1234',
            '--tlsservername', 'kimage-clean-builder.kimage-system.svc', '--tlscacert', '/builder-credentials/ca.crt',
            '--tlscert', '/builder-credentials/client.crt', '--tlskey', '/builder-credentials/client.key',
            'du', '--format', '{{json .}}']
    events = [json.loads(line) for line in call(['exec', client, '-c', 'client', '--'] + args).splitlines() if line]
    records = [record for event in events for record in (event if isinstance(event, list) else [event])]
    for record in records:
        description = record.get('Description', record.get('description', ''))
        if record.get('InUse', record.get('inUse')) and 'probe' in description and 'wait' in description:
            return image
    return None


image = until(submitted, 'actual in-flight engine solve')
old_id = image['status']['build']['snapshotId']
worker = get('pod', 'kimage-clean-builder', 'kimage-system')
old_worker_uid = worker['metadata']['uid']
print('actual in-flight solve observed; exercising', a.action, flush=True)
if a.action == 'generation':
    fast = 'FROM scratch\nCOPY probe /probe\nENTRYPOINT ["/probe"]\n'
    call(['patch', 'image', a.case, '--type=merge', '-p', json.dumps({'spec': {'source': {'build': {'definition': {'inline': fast}}}}})])
elif a.action == 'input':
    maps = json.loads(call(['get', 'configmaps', '-l', 'kimage.dev/build-snapshot=' + old_id[:40], '-o', 'json']))['items']
    frozen = next(m for m in maps if '-volume-' in m['metadata']['name'])
    assert frozen['immutable'] is True
    r = subprocess.run(b + ['-n', ns, 'patch', 'configmap', frozen['metadata']['name'], '--dry-run=server', '--type=merge', '-p', json.dumps(dict(binaryData=dict(probe=base64.b64encode(b'changed').decode())))], capture_output=True, timeout=65)
    assert r.returncode != 0 and b'immutable' in r.stderr.lower()
    call(['patch', 'configmap', source, '--type=merge', '-p', json.dumps(dict(data=dict(message=a.case)))])
elif a.action == 'delete':
    call(['delete', 'image', a.case, '--wait=false'])
else:
    operator = get('pod', 'kimage-clean-operator', 'kimage-system')
    operator['metadata'] = dict(name='kimage-clean-operator', labels=operator['metadata']['labels'])
    operator.pop('status', None)
    call(['delete', 'pod', 'kimage-clean-operator', '--grace-period=0', '--force', '--wait=true', '--timeout=60s'], namespace='kimage-system')
    create(operator, 'kimage-system')


def fenced():
    pod = get('pod', 'kimage-clean-builder', 'kimage-system')
    return pod if pod['metadata']['uid'] != old_worker_uid else None


# A brief absent-worker interval is expected during replacement.
end = time.monotonic() + 180
replacement = None
while time.monotonic() < end:
    raw = call(['get', 'pod', 'kimage-clean-builder', '--ignore-not-found', '-o', 'json'], namespace='kimage-system')
    if raw and json.loads(raw)['metadata']['uid'] != old_worker_uid:
        replacement = json.loads(raw)
        break
    time.sleep(2)
assert replacement, 'submitted worker was not fenced'
if a.action == 'delete':
    until(lambda: not call(['get', 'image', a.case, '--ignore-not-found', '-o', 'name']), 'Image finalizer completion')
    releases = json.loads(call(['get', 'imagereleases', '-o', 'json']))['items']
    assert not any(r['spec']['imageUid'] == image['metadata']['uid'] for r in releases)
    evidence = dict(action=a.action, imageUid=image['metadata']['uid'], activeEngineSolveObserved=True,
                    oldWorkerUid=old_worker_uid, replacementWorkerUid=replacement['metadata']['uid'],
                    finalizerCompleted=True, noStaleRelease=True)
else:
    def ready():
        current = get('image', a.case)
        status = current.get('status', {})
        return current if status.get('observedGeneration') == current['metadata']['generation'] and any(c['type'] == 'Ready' and c['status'] == 'True' for c in status.get('conditions', [])) else None
    current = until(ready, 'replacement build publication', 480)
    release = get('imagerelease', current['status']['releaseRef'])
    assert release['status']['verified']
    assert release['spec']['sourceGeneration'] == current['metadata']['generation']
    assert release['spec']['provenance']['execution']['attemptId'] != old_id
    assert release['spec']['provenance']['execution']['workerUid'] != old_worker_uid
    evidence = dict(action=a.action, imageUid=image['metadata']['uid'], activeEngineSolveObserved=True,
                    oldWorkerUid=old_worker_uid, replacementWorkerUid=replacement['metadata']['uid'],
                    oldAttemptNotPublished=True, currentGenerationPublished=True, releaseUid=release['metadata']['uid'])
    if a.action == 'input':
        recorded = next(i for i in release['spec']['provenance']['inputs'] if i['kind'] == 'ConfigMap' and i['name'] == source)
        assert recorded['resourceVersion'] == get('configmap', source)['metadata']['resourceVersion']
        evidence.update(frozenSnapshotMutationRefused=True, sourceChangeDuringRunCanceled=True, newSourceVersionPublished=True)
a.output.write_text(json.dumps(evidence, indent=2) + '\n')
print('actual Direct lifecycle case', a.action, 'passed', flush=True)

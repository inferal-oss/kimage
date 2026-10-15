#!/usr/bin/env python3
"""Frozen mutable ConfigMap inputs and immutable version conflict acceptance."""
import argparse
import hashlib
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
assert a.case.startswith('kimage-version-') and a.case.replace('-', '').isalnum()
b = ['kubectl', '--kubeconfig', str(a.kubeconfig), '--request-timeout=45s', '-n', 'kimage-app']


def call(args, body=None):
    r = subprocess.run(b + args, input=None if body is None else json.dumps(body).encode(), capture_output=True, timeout=65)
    if r.returncode:
        raise RuntimeError(r.stderr.decode()[:1500])
    return r.stdout


def get(kind, name):
    return json.loads(call(['get', kind, name, '-o', 'json']))


def until(predicate, label, seconds=480):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        result = predicate()
        if result:
            return result
        time.sleep(2)
    raise RuntimeError(label + ' deadline')


def ready():
    image = get('image', a.case)
    status = image.get('status', {})
    return image if status.get('observedGeneration') == image['metadata']['generation'] and any(c['type'] == 'Ready' and c['status'] == 'True' for c in status.get('conditions', [])) else None


source = a.case + '-source'
first_value = hashlib.sha256((a.case + '/first').encode()).hexdigest()
second_value = hashlib.sha256((a.case + '/second').encode()).hexdigest()
existing = call(['get', 'configmap', source, '--ignore-not-found', '-o', 'json'])
build = dict(format='Containerfile', execution=dict(strategy='Direct', builderRef='default'),
    definition=dict(inline='FROM scratch\nCOPY message /message\n'), inputs=dict(contextPath='/workspace/context',
        volumes=[dict(name='source', configMap=dict(name=source))],
        volumeMounts=[dict(name='source', mountPath='/workspace/context', readOnly=True)]))
if not existing:
    call(['create', '-f', '-'], dict(apiVersion='v1', kind='ConfigMap', metadata=dict(name=source), data=dict(message=first_value)))
    call(['create', '-f', '-'], dict(apiVersion='kimage.dev/v1alpha1', kind='Image', metadata=dict(name=a.case), spec=dict(
        version='1.0.0', platform=dict(os='linux', architecture='amd64'), source=dict(build=build))))
    image = until(ready, 'initial version')
    first_release = get('imagerelease', image['status']['releaseRef'])
else:
    image = get('image', a.case)
    assert image['spec']['version'] == '1.0.0', 'choose a new case after a completed run'
    releases = json.loads(call(['get', 'imagereleases', '-o', 'json']))['items']
    first_release = next(r for r in releases if r['spec']['imageUid'] == image['metadata']['uid'] and r['spec']['version'] == '1.0.0')
assert first_release['status']['verified']
source_before = get('configmap', source)
assert source_before['data']['message'] in [first_value, second_value]
if source_before['data']['message'] == first_value:
    call(['patch', 'configmap', source, '--type=merge', '-p', json.dumps(dict(data=dict(message=second_value)))])


def conflicted():
    image = get('image', a.case)
    return image if any(c['type'] == 'Ready' and c['status'] == 'False' and c.get('reason') == 'VersionConflict' for c in image.get('status', {}).get('conditions', [])) else None


current = until(conflicted, 'changed input same-version conflict')
assert current['metadata']['generation'] == image['metadata']['generation']
unchanged = get('imagerelease', first_release['metadata']['name'])
assert unchanged['metadata']['uid'] == first_release['metadata']['uid'] and unchanged['spec'] == first_release['spec']
call(['patch', 'image', a.case, '--type=merge', '-p', json.dumps(dict(spec=dict(version='1.0.1')))])
image = until(ready, 'new version')
second_release = get('imagerelease', image['status']['releaseRef'])
assert second_release['status']['verified']
assert second_release['metadata']['uid'] != first_release['metadata']['uid']
assert second_release['spec']['root']['digest'] != first_release['spec']['root']['digest']
assert second_release['spec']['snapshot'] != first_release['spec']['snapshot']
source_after = get('configmap', source)
recorded = next(i for i in second_release['spec']['provenance']['inputs'] if i['kind'] == 'ConfigMap' and i['name'] == source)
assert recorded['uid'] == source_after['metadata']['uid']
assert recorded['resourceVersion'] == source_after['metadata']['resourceVersion']
result = dict(imageUid=image['metadata']['uid'], firstReleaseUid=first_release['metadata']['uid'],
    secondReleaseUid=second_release['metadata']['uid'], firstRoot=first_release['spec']['root'], secondRoot=second_release['spec']['root'],
    changedInputObservedWithoutImageGenerationChange=True, sameVersionConflict=True, firstReleaseUnchanged=True,
    newVersionPublished=True, sourceUidStable=source_before['metadata']['uid'] == source_after['metadata']['uid'],
    changedResourceVersionRecorded=True)
a.output.parent.mkdir(parents=True, exist_ok=True)
a.output.write_text(json.dumps(result, indent=2) + '\n')
print('source watch, changed snapshot, immutable version conflict and new version passed', flush=True)

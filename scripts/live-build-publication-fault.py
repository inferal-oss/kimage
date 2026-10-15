#!/usr/bin/env python3
"""Recover a Direct candidate from retained bytes after API seal denial/restart."""
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
assert a.case.startswith('kimage-seal-') and a.case.replace('-', '').isalnum()
b = ['kubectl', '--kubeconfig', str(a.kubeconfig), '--request-timeout=45s']
ns = 'kimage-app'


def call(args, body=None, namespace=ns):
    args = b + ([] if namespace is None else ['-n', namespace]) + args
    r = subprocess.run(args, input=None if body is None else json.dumps(body).encode(), capture_output=True, timeout=100)
    if r.returncode:
        raise RuntimeError(r.stderr.decode()[:1800])
    return r.stdout


def get(kind, name, namespace=ns):
    return json.loads(call(['get', kind, name, '-o', 'json'], namespace=namespace))


def wait(predicate, label, seconds=480):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        result = predicate()
        if result:
            return result
        time.sleep(2)
    raise RuntimeError(label + ' deadline')


hook_name = 'kimage-clean-' + a.case.removeprefix('kimage-') + '-fault'
template = get('validatingwebhookconfiguration', 'kimage-clean-retention', None)['webhooks'][0]
template['name'] = hook_name + '.fixture.kimage.dev'
template['clientConfig']['service']['path'] = '/mutate'
template['rules'] = [dict(apiGroups=['kimage.dev'], apiVersions=['v1alpha1'], operations=['UPDATE'], resources=['imagereleases/status'], scope='Namespaced')]
template['matchConditions'] = [dict(name='owned-test-only', expression="object.spec.imageName == '" + a.case + "'")]
call(['create', '-f', '-'], dict(apiVersion='admissionregistration.k8s.io/v1', kind='ValidatingWebhookConfiguration',
    metadata=dict(name=hook_name, labels={'kimage.dev/installation': 'kimage-clean'}), webhooks=[template]), None)
active = True
try:
    source = a.case + '-input'
    call(['create', '-f', '-'], dict(apiVersion='v1', kind='ConfigMap', metadata=dict(name=source), data=dict(message=a.case)))
    build = dict(format='Containerfile', execution=dict(strategy='Direct', builderRef='default'),
        definition=dict(inline='FROM scratch\nCOPY message /message\n'), inputs=dict(contextPath='/workspace/context',
            volumes=[dict(name='source', configMap=dict(name=source))],
            volumeMounts=[dict(name='source', mountPath='/workspace/context', readOnly=True)]))
    call(['create', '-f', '-'], dict(apiVersion='kimage.dev/v1alpha1', kind='Image', metadata=dict(name=a.case), spec=dict(
        version='1.0.0', platform=dict(os='linux', architecture='amd64'), source=dict(build=build))))
    image = get('image', a.case)
    uid = image['metadata']['uid']

    def pending():
        releases = json.loads(call(['get', 'imagereleases', '-o', 'json']))['items']
        found = [r for r in releases if r['spec']['imageUid'] == uid]
        if not found:
            return None
        assert len(found) == 1 and not found[0].get('status', {}).get('verified', False)
        current = get('image', a.case)
        assert not any(c['type'] == 'Ready' and c['status'] == 'True' for c in current.get('status', {}).get('conditions', []))
        return found[0]

    candidate = wait(pending, 'unsealed candidate')
    current = get('image', a.case)
    assert {k: current['status']['operation']['root'][k] for k in ['mediaType', 'digest', 'size']} == {k: candidate['spec']['root'][k] for k in ['mediaType', 'digest', 'size']}
    assert current['status']['operation']['identity']['snapshot'] == candidate['spec']['snapshot']
    assert current['status']['build']['snapshotId'] == candidate['spec']['provenance']['execution']['attemptId']
    assert current['status']['operation']['phase'] in ['Publishing', 'Verified']
    old_attempt = candidate['spec']['provenance']['execution']['attemptId']
    call(['delete', 'pods', '-l', 'kimage.dev/image-uid=' + uid, '--grace-period=1', '--wait=true', '--timeout=90s'])
    call(['delete', 'configmap', source, '--wait=true', '--timeout=60s'])
    call(['delete', 'pod', 'kimage-clean-builder', '--grace-period=1', '--wait=true', '--timeout=90s'], namespace='kimage-system')
    operator = get('pod', 'kimage-clean-operator', 'kimage-system')
    operator['metadata'] = dict(name='kimage-clean-operator', labels=operator['metadata']['labels'])
    operator.pop('status', None)
    call(['delete', 'pod', 'kimage-clean-operator', '--grace-period=0', '--force', '--wait=true', '--timeout=90s'], namespace='kimage-system')
    call(['create', '-f', '-'], operator, 'kimage-system')
    wait(lambda: any(c['type'] == 'Ready' and c['status'] == 'True' for c in get('pod', 'kimage-clean-operator', 'kimage-system').get('status', {}).get('conditions', [])), 'operator restart')
    assert not get('imagerelease', candidate['metadata']['name']).get('status', {}).get('verified', False)
    call(['delete', 'validatingwebhookconfiguration', hook_name, '--wait=true', '--timeout=60s'], namespace=None)
    active = False

    def ready():
        current = get('image', a.case)
        return current if any(c['type'] == 'Ready' and c['status'] == 'True' for c in current.get('status', {}).get('conditions', [])) else None

    current = wait(ready, 'retained candidate recovery')
    release = get('imagerelease', current['status']['releaseRef'])
    assert release['metadata']['uid'] == candidate['metadata']['uid'] and release['status']['verified']
    assert release['spec'] == candidate['spec']
    assert release['spec']['provenance']['execution']['attemptId'] == old_attempt
    assert not call(['get', 'pod', 'kimage-clean-builder', '--ignore-not-found', '-o', 'name'], namespace='kimage-system')
    result = dict(imageUid=uid, releaseUid=release['metadata']['uid'], root=release['spec']['root'],
        apiSealDenied=True, noFalseReadiness=True, trustedRootCheckpointPreserved=True, operatorRestarted=True,
        sourceAndClientAbsentOnRecovery=True, workerAndCacheAbsentOnRecovery=True, releaseUidAndExecutionUnchanged=True,
        recoveredThroughRetainedGraph=True)
    a.output.parent.mkdir(parents=True, exist_ok=True)
    a.output.write_text(json.dumps(result, indent=2) + '\n')
    print('Direct candidate recovers from retained bytes after API denial with no source/client/worker', flush=True)
finally:
    if active:
        call(['delete', 'validatingwebhookconfiguration', hook_name, '--ignore-not-found', '--wait=true', '--timeout=60s'], namespace=None)

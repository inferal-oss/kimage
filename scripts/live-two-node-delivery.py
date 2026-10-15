#!/usr/bin/env python3
"""Real uncached pulls and controller rescheduling across the two owned nodes."""
import argparse
import base64
import copy
import json
import pathlib
import subprocess
import time

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--kubeconfig', type=pathlib.Path, required=True)
p.add_argument('--probe', type=pathlib.Path, required=True)
p.add_argument('--case', required=True)
p.add_argument('--output', type=pathlib.Path, required=True)
a = p.parse_args()
assert 'server: https://127.0.0.1:27643' in a.kubeconfig.read_text()
assert a.case.startswith('kimage-two-') and a.case.replace('-', '').isalnum()
b = ['kubectl', '--kubeconfig', str(a.kubeconfig), '--request-timeout=45s']
ns = 'kimage-app'


def call(args, body=None, namespace=ns):
    result = subprocess.run(b + ['-n', namespace] + args, input=None if body is None else json.dumps(body).encode(), capture_output=True, timeout=90)
    if result.returncode:
        raise RuntimeError(result.stderr.decode()[:1800])
    return result.stdout


def get(kind, name, namespace=ns):
    return json.loads(call(['get', kind, name, '-o', 'json'], namespace=namespace))


def ready(value):
    return any(c['type'] == 'Ready' and c['status'] == 'True' for c in value.get('status', {}).get('conditions', []))


def wait(predicate, label, seconds=600):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        value = predicate()
        if value:
            return value
        time.sleep(2)
    raise RuntimeError(label + ' deadline')


probe = a.probe.read_bytes()
assert len(probe) <= 512 * 1024
source = a.case + '-context'
call(['create', '-f', '-'], dict(apiVersion='v1', kind='ConfigMap', metadata=dict(name=source),
    data=dict(message=a.case), binaryData=dict(probe=base64.b64encode(probe).decode())))
build = dict(format='Containerfile', definition=dict(inline='FROM scratch\nCOPY probe /probe\nCOPY message /message\nENTRYPOINT ["/probe"]\n'),
    execution=dict(strategy='Direct', builderRef='default'), inputs=dict(contextPath='/workspace/context',
        volumes=[dict(name='context', configMap=dict(name=source, items=[dict(key='probe', path='probe', mode=365), dict(key='message', path='message', mode=292)]))],
        volumeMounts=[dict(name='context', mountPath='/workspace/context', readOnly=True)]))
call(['create', '-f', '-'], dict(apiVersion='kimage.dev/v1alpha1', kind='Image', metadata=dict(name=a.case), spec=dict(
    version='1.0.0', platform=dict(os='linux', architecture='amd64'), source=dict(build=build))))
image = wait(lambda: (lambda v: v if ready(v) else None)(get('image', a.case)), 'published two-node artifact')
release = get('imagerelease', image['status']['releaseRef'])
root = release['spec']['root']['digest']
assert release['status']['verified']
cache = {}
for node, directory in [('kimage-clean', '.local/qemu-routed'), ('kimage-clean-2', '.local/qemu-second')]:
    value = json.loads(subprocess.check_output(['python3', 'scripts/isolated-fixture.py', 'request', '--directory', directory, '--operation', 'images'], timeout=130))
    assert not any(root in reference for entry in value.get('images', []) for reference in entry.get('repoDigests', []))
    cache[node] = dict(rootAbsentBeforeWorkload=True)
template = dict(metadata=dict(labels={'kimage.dev/delivery-case': a.case}, annotations={'fixture.kimage.dev/preserve': 'original'}), spec=dict(
    nodeSelector={'kubernetes.io/hostname': 'kimage-clean'}, automountServiceAccountToken=False,
    affinity=dict(nodeAffinity=dict(requiredDuringSchedulingIgnoredDuringExecution=dict(nodeSelectorTerms=[
        dict(matchExpressions=[dict(key='kubernetes.io/os', operator='In', values=['linux'])]),
        dict(matchExpressions=[dict(key='kubernetes.io/os', operator='In', values=['unavailable'])])]))),
    containers=[dict(name='app', image='platform://kimage-app/' + a.case + ':1.0.0', imagePullPolicy='IfNotPresent',
        resources=dict(requests=dict(cpu='5m', memory='8Mi'), limits=dict(cpu='100m', memory='32Mi')))]))
deployment = dict(apiVersion='apps/v1', kind='Deployment', metadata=dict(name=a.case), spec=dict(replicas=1,
    selector=dict(matchLabels={'kimage.dev/delivery-case': a.case}), strategy=dict(type='Recreate'), template=template))
call(['create', '-f', '-'], deployment)


def workload(node):
    pods = json.loads(call(['get', 'pods', '-l', 'kimage.dev/delivery-case=' + a.case, '-o', 'json']))['items']
    found = [pod for pod in pods if pod['spec'].get('nodeName') == node and not pod['metadata'].get('deletionTimestamp') and ready(pod)]
    return found[0] if found else None


first = wait(lambda: workload('kimage-clean'), 'first-node normal pull')
call(['patch', 'deployment', a.case, '--type=merge', '-p', json.dumps({'spec': {'template': {'spec': {'nodeSelector': {'kubernetes.io/hostname': 'kimage-clean-2'}}}}})])
second = wait(lambda: workload('kimage-clean-2'), 'second-node rescheduled normal pull')
assert first['metadata']['uid'] != second['metadata']['uid']
for pod, node in [(first, 'kimage-clean'), (second, 'kimage-clean-2')]:
    assert pod['spec']['containers'][0]['image'].endswith('@' + root)
    assert pod['spec']['containers'][0]['imagePullPolicy'] == 'IfNotPresent'
    assert pod['spec']['nodeSelector'] == {'kubernetes.io/hostname': node}
    assert pod['metadata']['annotations']['fixture.kimage.dev/preserve'] == 'original'
    terms = pod['spec']['affinity']['nodeAffinity']['requiredDuringSchedulingIgnoredDuringExecution']['nodeSelectorTerms']
    assert len(terms) == 4
    original_terms = template['spec']['affinity']['nodeAffinity']['requiredDuringSchedulingIgnoredDuringExecution']['nodeSelectorTerms']
    for index, term in enumerate(terms):
        assert all(expr in term['matchExpressions'] for expr in original_terms[index // 2]['matchExpressions'])
        assert all(len(field['values']) == 1 for field in term['matchFields'])
    assert all({field['values'][0] for term in terms[index:index + 2] for field in term['matchFields'] if field['key'] == 'metadata.name'} == {'kimage-clean', 'kimage-clean-2'} for index in [0, 2])
current_release = get('imagerelease', release['metadata']['name'])
assert current_release['metadata']['uid'] == release['metadata']['uid'] and current_release['spec'] == release['spec']
assert call(['logs', second['metadata']['name']]).decode().strip() == 'kimage normal pull'
nodes = json.loads(call(['get', 'nodes', '-o', 'json']))['items']
node2_uid = next(node['metadata']['uid'] for node in nodes if node['metadata']['name'] == 'kimage-clean-2')
import hashlib
scope = 'kimage-clean-n-' + hashlib.sha256(node2_uid.encode()).hexdigest()[:16]
for proxy in ['kimage-clean-proxy', scope + '-proxy']:
    assert root.encode() in call(['logs', proxy, '--tail=3000'], namespace='kimage-system')

# Administrative revocation must withdraw only that node, even while its Pod
# still reports Ready. Restore through a guarded write before leaving the test.
record_name = scope + '-endpoint'
record = get('configmap', record_name, 'kimage-system')
original = record['data']['endpoint.json']
revoked = json.loads(original)
revoked['proxyImages'] = ['127.0.0.1:17641/kimage-system/system-proxy@sha256:' + '0' * 64]
revoked = json.dumps(revoked)
call(['patch', 'configmap', record_name, '--type=merge', '-p', json.dumps(dict(metadata=dict(uid=record['metadata']['uid'], resourceVersion=record['metadata']['resourceVersion']), data={'endpoint.json': revoked}))], namespace='kimage-system')
try:
    dry = dict(apiVersion='v1', kind='Pod', metadata=dict(name=a.case + '-ineligible'), spec=copy.deepcopy(template['spec']))
    dry['spec']['nodeSelector'] = {'kubernetes.io/hostname': 'kimage-clean-2'}
    value = json.loads(call(['create', '--dry-run=server', '-f', '-', '-o', 'json'], dry))
    terms = value['spec']['affinity']['nodeAffinity']['requiredDuringSchedulingIgnoredDuringExecution']['nodeSelectorTerms']
    assert all(field['values'] == ['kimage-clean'] for term in terms for field in term['matchFields'] if field['key'] == 'metadata.name')
finally:
    current = get('configmap', record_name, 'kimage-system')
    assert current['data']['endpoint.json'] == revoked
    call(['patch', 'configmap', record_name, '--type=merge', '-p', json.dumps(dict(metadata=dict(uid=current['metadata']['uid'], resourceVersion=current['metadata']['resourceVersion']), data={'endpoint.json': original}))], namespace='kimage-system')
result = dict(imageUid=image['metadata']['uid'], releaseUid=release['metadata']['uid'], root=release['spec']['root'],
    firstPodUid=first['metadata']['uid'], secondPodUid=second['metadata']['uid'], firstNode='kimage-clean', secondNode='kimage-clean-2',
    cache=cache, bothProxyRootRequestsObserved=True, normalIfNotPresentPulls=True, controllerRescheduled=True,
    releaseAndExecutionUnchanged=True, userSelectorAffinityAndAnnotationPreserved=True, ineligibleNodeExcludedDespiteReadyPod=True)
a.output.parent.mkdir(parents=True, exist_ok=True)
a.output.write_text(json.dumps(result, indent=2) + '\n')
print('two uncached normal pulls, controller rescheduling and node-projection eligibility passed', flush=True)

#!/usr/bin/env python3
"""Prove retained readiness/running containers survive registry downtime, while
an uncached ordinary pull waits and recovers when the same registry returns.
"""
import argparse
import base64
import json
import pathlib
import subprocess
import time

p = argparse.ArgumentParser(description=__doc__)
for name in ['kubeconfig', 'probe', 'wait-probe', 'output']:
    p.add_argument('--' + name, type=pathlib.Path, required=True)
p.add_argument('--case', required=True)
a = p.parse_args()
assert 'server: https://127.0.0.1:27643' in a.kubeconfig.read_text()
assert a.case.startswith('kimage-downtime-') and a.case.replace('-', '').isalnum()
b = ['kubectl', '--kubeconfig', str(a.kubeconfig), '--request-timeout=45s']


def call(args, body=None, ns='kimage-app'):
    result = subprocess.run(b + ['-n', ns] + args, input=None if body is None else json.dumps(body).encode(), capture_output=True, timeout=100)
    if result.returncode:
        raise RuntimeError(result.stderr.decode()[:1800])
    return result.stdout


def get(kind, name, ns='kimage-app'):
    return json.loads(call(['get', kind, name, '-o', 'json'], ns=ns))


def ready(obj):
    return any(c['type'] == 'Ready' and c['status'] == 'True' for c in obj.get('status', {}).get('conditions', []))


def wait(predicate, label, seconds=600):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        value = predicate()
        if value:
            return value
        time.sleep(2)
    raise RuntimeError(label + ' deadline')


source = a.case + '-context'
call(['create', '-f', '-'], dict(apiVersion='v1', kind='ConfigMap', metadata=dict(name=source), data=dict(message=a.case), binaryData=dict(probe=base64.b64encode(a.probe.read_bytes()).decode())))
build = dict(format='Containerfile', definition=dict(inline='FROM scratch\nCOPY probe /probe\nCOPY message /message\nENTRYPOINT ["/probe"]\n'), execution=dict(strategy='Direct', builderRef='default'), inputs=dict(contextPath='/workspace/context', volumes=[dict(name='context', configMap=dict(name=source, items=[dict(key='probe', path='probe', mode=365), dict(key='message', path='message', mode=292)]))], volumeMounts=[dict(name='context', mountPath='/workspace/context', readOnly=True)]))
call(['create', '-f', '-'], dict(apiVersion='kimage.dev/v1alpha1', kind='Image', metadata=dict(name=a.case), spec=dict(version='1.0.0', platform=dict(os='linux', architecture='amd64'), source=dict(build=build))))
image = wait(lambda: (lambda v: v if ready(v) else None)(get('image', a.case)), 'fresh retained publication')
release = get('imagerelease', image['status']['releaseRef'])
root = release['spec']['root']['digest']
cache = json.loads(subprocess.check_output(['python3', 'scripts/isolated-fixture.py', 'request', '--directory', '.local/qemu-second', '--operation', 'images'], timeout=130))
assert not any(root in ref for item in cache['images'] for ref in item.get('repoDigests', []))
helper = a.case + '-delay'
call(['create', '-f', '-'], dict(apiVersion='v1', kind='ConfigMap', metadata=dict(name=helper), binaryData=dict(probe=base64.b64encode(a.wait_probe.read_bytes()).decode())))
carrier = 'docker.io/rancher/mirrored-pause@sha256:f548e0e8e3dc1896ca956272154dde3314e8cc4fde0a57577ee9fa1c63f5baf4'
podname = a.case + '-pull'
call(['create', '-f', '-'], dict(apiVersion='v1', kind='Pod', metadata=dict(name=podname), spec=dict(nodeSelector={'kubernetes.io/hostname': 'kimage-clean-2'}, automountServiceAccountToken=False, volumes=[dict(name='helper', configMap=dict(name=helper, defaultMode=365))], initContainers=[dict(name='delay', image=carrier, imagePullPolicy='Never', command=['/helper/probe', 'wait', '30'], securityContext=dict(runAsUser=0, allowPrivilegeEscalation=False, capabilities=dict(drop=['ALL'])), volumeMounts=[dict(name='helper', mountPath='/helper', readOnly=True)])], containers=[dict(name='app', image='platform://kimage-app/' + a.case + ':1.0.0', imagePullPolicy='IfNotPresent')])) )
wait(lambda: any('running' in c.get('state', {}) for c in get('pod', podname).get('status', {}).get('initContainerStatuses', [])), 'delay init running')
keep = next(pod for pod in json.loads(call(['get', 'pods', '-l', 'kimage.dev/delivery-case=kimage-two-a2', '-o', 'json']))['items'] if ready(pod))
keep_id = keep['status']['containerStatuses'][0]['containerID']
registry = get('pod', 'kimage-clean-registry', 'kimage-system')
assert registry['metadata']['labels']['kimage.dev/installation'] == 'kimage-clean'
template = dict(apiVersion='v1', kind='Pod', metadata={key: registry['metadata'][key] for key in ['name', 'labels', 'annotations'] if key in registry['metadata']}, spec=registry['spec'])
call(['delete', 'pod', 'kimage-clean-registry', '--wait=true', '--timeout=90s'], ns='kimage-system')
blocked = None
try:
    def blocked_pull():
        value = get('pod', podname)
        return value if any(c.get('state', {}).get('waiting', {}).get('reason') in ['ErrImagePull', 'ImagePullBackOff'] for c in value.get('status', {}).get('containerStatuses', [])) else None
    blocked = wait(blocked_pull, 'uncached pull blocked by downtime', 240)
    current = get('pod', keep['metadata']['name'])
    assert ready(current) and current['metadata']['uid'] == keep['metadata']['uid'] and current['status']['containerStatuses'][0]['containerID'] == keep_id
    assert ready(get('image', a.case)) and get('imagerelease', release['metadata']['name'])['spec'] == release['spec']
finally:
    call(['create', '-f', '-'], template, ns='kimage-system')
    wait(lambda: ready(get('pod', 'kimage-clean-registry', 'kimage-system')), 'registry restored')
restored = wait(lambda: (lambda v: v if ready(v) else None)(get('pod', podname)), 'ordinary pull retry', 420)
assert restored['metadata']['uid'] == blocked['metadata']['uid'] and restored['spec']['containers'][0]['image'].endswith('@' + root)
assert get('imagerelease', release['metadata']['name'])['spec'] == release['spec']
result = dict(root=release['spec']['root'], releaseUid=release['metadata']['uid'], rootAbsentBeforePull=True, runningContainerSurvived=True, retainedReadinessPreserved=True, uncachedPullBlocked=True, samePodNormalRetryRecovered=True, noRebuild=True)
a.output.write_text(json.dumps(result, indent=2) + '\n')
print('registry downtime preserves retained readiness and running containers; uncached pull retries normally', flush=True)

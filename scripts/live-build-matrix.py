#!/usr/bin/env python3
"""Real mounted-definition, secret-session and cache checks on the owned fixture."""
import argparse
import base64
import copy
import json
import os
import pathlib
import subprocess
import time

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--kubeconfig', type=pathlib.Path, required=True)
p.add_argument('--probe', type=pathlib.Path, required=True)
p.add_argument('--output', type=pathlib.Path, required=True)
p.add_argument('--case', required=True)
a = p.parse_args()
assert 'server: https://127.0.0.1:27643' in a.kubeconfig.read_text()
assert a.case.startswith('kimage-matrix-') and a.case.replace('-', '').isalnum()
a.output.mkdir(parents=True, exist_ok=True)
b = ['kubectl', '--kubeconfig', str(a.kubeconfig), '--request-timeout=45s']
ns = 'kimage-app'


def call(args, body=None, namespace=ns):
    r = subprocess.run(b + ['-n', namespace] + args, input=None if body is None else json.dumps(body).encode(), capture_output=True, timeout=65)
    if r.returncode:
        raise RuntimeError(r.stderr.decode()[:2000])
    return r.stdout


def get(kind, name, namespace=ns):
    return json.loads(call(['get', kind, name, '-o', 'json'], namespace=namespace))


def create(body):
    name = body['metadata']['name']
    if not call(['get', body['kind'], name, '--ignore-not-found', '-o', 'name']):
        call(['create', '-f', '-'], body)


def published(name, timeout=900):
    end = time.monotonic() + timeout
    previous = None
    while time.monotonic() < end:
        image = get('image', name)
        conditions = image.get('status', {}).get('conditions', [])
        current = [(c['type'], c['status'], c.get('reason')) for c in conditions]
        if current != previous:
            print(name, current, flush=True)
            previous = current
        if image.get('status', {}).get('observedGeneration') == image['metadata']['generation'] and any(c['type'] == 'Ready' and c['status'] == 'True' for c in conditions):
            release = get('imagerelease', image['status']['releaseRef'])
            assert release['status']['verified']
            return image, release
        time.sleep(3)
    raise RuntimeError('publication deadline: ' + name)


private_file = a.output / 'private.json'
if not private_file.exists():
    private_file.write_text(json.dumps(dict(message=os.urandom(16).hex(), token=os.urandom(16).hex(), marker=os.urandom(16).hex())))
    private_file.chmod(0o600)
private = json.loads(private_file.read_text())
source = a.case + '-context'
definition_name = a.case + '-definition'
secret_name = a.case + '-private'
probe = a.probe.read_bytes()
assert len(probe) <= 512 * 1024
create(dict(apiVersion='v1', kind='ConfigMap', metadata=dict(name=source),
            data=dict(message=private['message']), binaryData=dict(probe=base64.b64encode(probe).decode())))
definition = ('FROM scratch\nCOPY probe /probe\n'
              'RUN --mount=type=bind,source=.,target=/bound --mount=type=secret,id=token ["/probe","run"]\n'
              'ENTRYPOINT ["/probe"]\n')
create(dict(apiVersion='v1', kind='ConfigMap', metadata=dict(name=definition_name), data=dict(recipe=definition)))
create(dict(apiVersion='v1', kind='Secret', metadata=dict(name=secret_name), type='Opaque',
            data={k: base64.b64encode(v.encode()).decode() for k, v in dict(token=private['token'], recipe='# ' + private['marker'] + '\n' + definition).items()}))
inputs = dict(contextPath='/workspace/context/nested', volumes=[
    dict(name='context', configMap=dict(name=source, items=[dict(key='probe', path='nested/probe', mode=365), dict(key='message', path='nested/message', mode=292)])),
    dict(name='definition', configMap=dict(name=definition_name, items=[dict(key='recipe', path='recipes/Containerfile', mode=292)])),
    dict(name='private', secret=dict(name=secret_name))], volumeMounts=[
        dict(name='context', mountPath='/workspace/context', readOnly=True),
        dict(name='definition', mountPath='/workspace/inputs/Containerfile', subPath='recipes/Containerfile', readOnly=True),
        dict(name='private', mountPath='/run/kimage-build-secrets', readOnly=True)])
build = dict(format='Containerfile', execution=dict(strategy='Direct', builderRef='default'),
             definition=dict(fromFile=dict(path='/workspace/inputs/Containerfile')), inputs=inputs,
             buildSecrets=[dict(id='token', sourceFile='/run/kimage-build-secrets/token')], cache=dict(mode='Local'))
evidence = {}
receipt_path = a.output / 'evidence.json'
if receipt_path.exists():
    evidence = json.loads(receipt_path.read_text())


def run(label, declaration):
    name = a.case + '-' + label
    create(dict(apiVersion='kimage.dev/v1alpha1', kind='Image', metadata=dict(name=name), spec=dict(
        version='1.0.0', source=dict(build=declaration), platform=dict(os='linux', architecture='amd64'))))
    image, release = published(name)
    execution = release['spec']['provenance']['execution']
    assert execution['engineWorkerId'] and execution['engineOperationId']
    evidence[label] = dict(imageUid=image['metadata']['uid'], releaseUid=release['metadata']['uid'],
                           root=release['spec']['root'], execution=execution)
    receipt_path.write_text(json.dumps(evidence, indent=2) + '\n')
    return image, release


_, first = run('mounted', build)
assert first['spec']['provenance']['execution']['executedSteps'] >= 1
_, cached = run('cached', build)
assert cached['spec']['provenance']['execution']['cachedExecutionSteps'] >= 1
disabled = copy.deepcopy(build)
disabled['cache']['mode'] = 'Disabled'
_, uncached = run('disabled', disabled)
assert uncached['spec']['provenance']['execution']['cachedExecutionSteps'] == 0
assert uncached['spec']['provenance']['execution']['executedSteps'] >= 1
secret_definition = copy.deepcopy(build)
secret_definition['definition']['fromFile']['path'] = '/run/kimage-build-secrets/recipe'
_, secret_release = run('secret-definition', secret_definition)
public = json.dumps(secret_release)
assert private['marker'] not in public and private['token'] not in public
worker_logs = call(['logs', 'kimage-clean-builder', '--tail=2000'], namespace='kimage-system')
operator_logs = call(['logs', 'kimage-clean-operator', '--tail=2000'], namespace='kimage-system')
assert private['marker'].encode() not in worker_logs + operator_logs
assert private['token'].encode() not in worker_logs + operator_logs

# A separate provider-cached carrier stages this test's PVC through a regular
# Pod filesystem mount. No host/runtime socket or image-store injection is used.
pvc = a.case + '-source'
stage = a.case + '-producer'
fixture = a.case + '-fixture'
create(dict(apiVersion='v1', kind='PersistentVolumeClaim', metadata=dict(name=pvc), spec=dict(
    storageClassName='local-path', accessModes=['ReadWriteOnce'], resources=dict(requests=dict(storage='1Gi')))))
create(dict(apiVersion='v1', kind='ConfigMap', metadata=dict(name=fixture), data=dict(recipe=definition), binaryData=dict(probe=base64.b64encode(probe).decode())))
create(dict(apiVersion='v1', kind='Pod', metadata=dict(name=stage), spec=dict(
    nodeSelector={'kubernetes.io/hostname': 'kimage-clean'}, automountServiceAccountToken=False, restartPolicy='Never',
    containers=[dict(name='producer', image='docker.io/rancher/mirrored-pause@sha256:f548e0e8e3dc1896ca956272154dde3314e8cc4fde0a57577ee9fa1c63f5baf4', imagePullPolicy='IfNotPresent',
        command=['/fixture/probe', 'stage', private['message'], '120'],
        securityContext=dict(runAsUser=0, allowPrivilegeEscalation=False, readOnlyRootFilesystem=True, capabilities=dict(drop=['ALL'])),
        resources=dict(requests=dict(cpu='10m', memory='16Mi'), limits=dict(cpu='100m', memory='32Mi')),
        volumeMounts=[dict(name='fixture', mountPath='/fixture', readOnly=True), dict(name='destination', mountPath='/destination')])],
    volumes=[dict(name='fixture', configMap=dict(name=fixture, defaultMode=365)), dict(name='destination', persistentVolumeClaim=dict(claimName=pvc))])))
end = time.monotonic() + 120
while time.monotonic() < end:
    if get('pod', stage).get('status', {}).get('phase') == 'Running':
        break
    time.sleep(1)
else:
    raise RuntimeError('PVC producer did not start')
pvc_build = copy.deepcopy(build)
pvc_build['inputs']['contextPath'] = '/workspace/context'
pvc_build['inputs']['producerHandoff'] = True
pvc_build['inputs']['volumes'][0] = dict(name='context', persistentVolumeClaim=dict(claimName=pvc, readOnly=True))
pvc_build['inputs']['volumeMounts'][0]['subPath'] = 'subdir'
pvc_image = a.case + '-pvc'
create(dict(apiVersion='kimage.dev/v1alpha1', kind='Image', metadata=dict(name=pvc_image), spec=dict(
    version='1.0.0', source=dict(build=pvc_build), platform=dict(os='linux', architecture='amd64'))))
end = time.monotonic() + 75
while time.monotonic() < end:
    v = get('image', pvc_image)
    if any(c.get('reason') == 'PvcWriterNotQuiescent' for c in v.get('status', {}).get('conditions', [])):
        break
    time.sleep(2)
else:
    raise RuntimeError('live PVC writer was not deferred')
call(['delete', 'pod', stage, '--grace-period=1', '--wait=true', '--timeout=60s'])
_, pvc_release = run('pvc', pvc_build)
assert {'PersistentVolume', 'PersistentVolumeClaim'}.issubset({i['kind'] for i in pvc_release['spec']['provenance']['inputs']})
pvc_definition = copy.deepcopy(pvc_build)
pvc_definition['definition']['fromFile']['path'] = '/workspace/inputs/pvc/Containerfile'
pvc_definition['inputs']['volumeMounts'].append(dict(name='context', mountPath='/workspace/inputs/pvc', subPath='subdir', readOnly=True))
_, pvc_definition_release = run('pvc-definition', pvc_definition)
assert pvc_definition_release['spec']['provenance']['execution']['engineOperationId']

# Placement/cache policy changes must keep the immutable release and solve ID.
name = a.case + '-mounted'
old = evidence['mounted']
call(['patch', 'image', name, '--type=merge', '-p', json.dumps({'spec': {'source': {'build': {'cache': {'mode': 'Disabled'}}}}})])
image, release = published(name)
assert release['metadata']['uid'] == old['releaseUid']
assert release['spec']['provenance']['execution'] == old['execution']
evidence['checks'] = dict(mountedConfigMapDefinition=True, subPathDefinition=True, contextRootIndependent=True,
                         binaryKeyMappingAndModes=True, ordinaryRunBindMount=True, runSecretMount=True,
                         localCacheHit=True, disabledExecutes=True, secretDefinitionOmittedFromStatusAndLogs=True,
                         cachePolicyChangeRetainsRelease=True, pvcSubPathContext=True, knownWriterDeferred=True,
                         pvcAndVolumeUidsRecorded=True, pvcMountedDefinition=True)
receipt_path.write_text(json.dumps(evidence, indent=2) + '\n')
print('mounted definitions, RUN mounts, private definition and real cache modes passed', flush=True)

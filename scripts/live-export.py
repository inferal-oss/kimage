#!/usr/bin/env python3
"""Actual independent-registry export acceptance on the two owned nodes."""
import argparse
import base64
import copy
import json
import os
import pathlib
import secrets
import subprocess
import time

p = argparse.ArgumentParser(description=__doc__)
for key in ['kubeconfig', 'target', 'installer', 'probe', 'output']:
    p.add_argument('--' + key, type=pathlib.Path, required=True)
p.add_argument('--case', required=True)
a = p.parse_args()
assert 'server: https://127.0.0.1:27643' in a.kubeconfig.read_text()
assert a.case.startswith('kimage-export-') and a.case.replace('-', '').isalnum()
a.output.mkdir(parents=True, exist_ok=True, mode=0o700)
metadata = json.loads((a.target / 'metadata.json').read_text())
authority = metadata['origin'].removeprefix('https://').removesuffix('/')
repository = metadata['repository'] + '/' + a.case
env = dict(os.environ, KUBECONFIG=str(a.kubeconfig.resolve()))
b = ['kubectl', '--kubeconfig', str(a.kubeconfig), '--request-timeout=45s']
credential_name = a.case + '-credentials'
ca_name = a.case + '-ca'


def call(args, body=None, ns='kimage-app'):
    result = subprocess.run(b + ['-n', ns] + args, input=None if body is None else json.dumps(body).encode(), capture_output=True, timeout=130)
    if result.returncode:
        if body and body.get('kind') == 'Secret':
            raise RuntimeError('fixture Secret write failed')
        raise RuntimeError(result.stderr.decode()[:1800])
    return result.stdout


def get(kind, name, ns='kimage-app'):
    return json.loads(call(['get', kind, name, '-o', 'json'], ns=ns))


def wait(predicate, label, seconds=600):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        value = predicate()
        if value:
            return value
        time.sleep(0.3)
    raise RuntimeError(label + ' deadline')


def ready(value):
    return any(c['type'] == 'Ready' and c['status'] == 'True' for c in value.get('status', {}).get('conditions', []))


def status(name, target='primary', phase='Complete'):
    value = get('image', name)
    item = next((s for s in value.get('status', {}).get('exports', []) if s['name'] == target and s['phase'] == phase), None)
    return (value, item) if item else None


def export_target(name='primary', tag='1.0.0'):
    return dict(name=name, registry=dict(repository=repository, tags=[tag], credentialsSecretRef=dict(name=credential_name), caSecretRef=dict(name=ca_name), overwritePolicy='Never'))


def patch_exports(name, targets):
    current = get('image', name)
    call(['patch', 'image', name, '--type=merge', '-p', json.dumps(dict(metadata=dict(uid=current['metadata']['uid'], resourceVersion=current['metadata']['resourceVersion']), spec=dict(exports=targets)))])


def create_image(name, exports=None):
    source = name + '-context'
    call(['create', '-f', '-'], dict(apiVersion='v1', kind='ConfigMap', metadata=dict(name=source), data=dict(message=name), binaryData=dict(probe=base64.b64encode(a.probe.read_bytes()).decode())))
    build = dict(format='Containerfile', definition=dict(inline='FROM scratch\nCOPY probe /probe\nCOPY message /message\nENTRYPOINT ["/probe"]\n'), execution=dict(strategy='Direct', builderRef='default'), inputs=dict(contextPath='/workspace/context', volumes=[dict(name='context', configMap=dict(name=source, items=[dict(key='probe', path='probe', mode=365), dict(key='message', path='message', mode=292)]))], volumeMounts=[dict(name='context', mountPath='/workspace/context', readOnly=True)]))
    spec = dict(version='1.0.0', platform=dict(os='linux', architecture='amd64'), source=dict(build=build))
    if exports is not None:
        spec['exports'] = exports
    call(['create', '-f', '-'], dict(apiVersion='kimage.dev/v1alpha1', kind='Image', metadata=dict(name=name), spec=spec))
    image = wait(lambda: (lambda v: v if ready(v) else None)(get('image', name)), 'internal publication ' + name)
    return image, get('imagerelease', image['status']['releaseRef'])


def verify_remote(release, label, tag='1.0.0'):
    rootfile = a.output / (label + '-root.json')
    rootfile.write_text(json.dumps(release['spec']['root']))
    # This independently downloads/verifies every original OCI byte through an
    # API tunnel, separately from the exporter's success acknowledgment.
    subprocess.run([str(a.installer.resolve()), 'verify-remote', '--root', str(rootfile.resolve()), '--tag', tag, '--namespace', metadata['namespace'], '--pod', metadata['pod'], '--origin', metadata['origin'], '--repository', repository.split('/', 1)[1], '--ca', str((a.target / 'credentials/ca.crt').resolve()), '--password-file', str((a.target / 'credentials/publisher-password').resolve())], env=env, check=True, timeout=600, stdout=subprocess.DEVNULL)


def restart_template(name, ns):
    pod = get('pod', name, ns)
    template = dict(apiVersion='v1', kind='Pod', metadata={key: pod['metadata'][key] for key in ['name', 'labels', 'annotations'] if key in pod['metadata']}, spec=pod['spec'])
    call(['delete', 'pod', name, '--grace-period=1', '--wait=true', '--timeout=90s'], ns=ns)
    return template


def restore(template, ns):
    call(['create', '-f', '-'], template, ns=ns)
    return wait(lambda: (lambda v: v if ready(v) else None)(get('pod', template['metadata']['name'], ns)), 'restore ' + template['metadata']['name'])


def credentials_body(password):
    value = dict(auths={authority: dict(auth=base64.b64encode(b'publisher:' + password).decode())})
    return {'.dockerconfigjson': base64.b64encode(json.dumps(value).encode()).decode()}


password = (a.target / 'credentials/publisher-password').read_bytes()
call(['create', '-f', '-'], dict(apiVersion='v1', kind='Secret', metadata=dict(name=credential_name), type='kubernetes.io/dockerconfigjson', data=credentials_body(password)))
call(['create', '-f', '-'], dict(apiVersion='v1', kind='Secret', metadata=dict(name=ca_name), type='Opaque', data={'ca.crt': base64.b64encode((a.target / 'credentials/ca.crt').read_bytes()).decode()}))
image, release = create_image(a.case)
assert not image.get('status', {}).get('exports')
# Retained export has neither its original source nor a live build controller.
call(['delete', 'configmap', a.case + '-context', '--wait=true', '--timeout=30s'])
operator = restart_template('kimage-clean-operator', 'kimage-system')
worker = restart_template('kimage-clean-builder', 'kimage-system')
result = dict(releaseUid=release['metadata']['uid'], root=release['spec']['root'], internallyPublishedWithoutExports=True)
try:
    patch_exports(a.case, [export_target()])
    first, first_status = wait(lambda: status(a.case), 'authenticated retained export')
    assert first_status['releaseUid'] == release['metadata']['uid'] and first_status['localDigest'] == first_status['remoteDigest'] == release['spec']['root']['digest']
    assert first_status['remoteReference'] == repository + '@' + release['spec']['root']['digest']
    verify_remote(release, 'first')
    result.update(authenticatedDigestPreservingExport=True, independentFullRemoteReadback=True, sourceWorkerAndBuildControllerAbsent=True)
    patch_exports(a.case, [export_target(), export_target('idempotent')])
    wait(lambda: status(a.case, 'idempotent'), 'same-digest tag idempotence')
    result['sameDigestTagIdempotent'] = True
    # Persisted selected identity must survive exporter process loss before ACK.
    patch_exports(a.case, [export_target('interrupted', 'interrupted')])
    wait(lambda: status(a.case, 'interrupted', 'Transferring'), 'observable export transfer checkpoint')
    exporter = restart_template('kimage-clean-exporter', 'kimage-system')
    restore(exporter, 'kimage-system')
    _, resumed = wait(lambda: status(a.case, 'interrupted'), 'exporter restart retry')
    assert resumed['releaseUid'] == release['metadata']['uid'] and resumed['remoteDigest'] == release['spec']['root']['digest']
    result['interruptedExporterResumedSameRetainedIdentity'] = True
    # Default Never must preserve the old alias when a different release asks.
finally:
    restore(worker, 'kimage-system')
    restore(operator, 'kimage-system')

conflict_image, conflict_release = create_image(a.case + '-conflict', [export_target('conflict')])
_, conflict = wait(lambda: status(a.case + '-conflict', 'conflict', 'TagConflict'), 'different digest tag conflict')
assert ready(get('image', a.case + '-conflict')) and conflict['localDigest'] == conflict_release['spec']['root']['digest']
verify_remote(release, 'conflict-original')
result['differentDigestTagConflictKeepsInternalReady'] = True

# Destination downtime must not gate a new build/publication/normal deployment.
target_template = restart_template(metadata['pod'], metadata['namespace'])
try:
    offline_image, offline_release = create_image(a.case + '-offline', [export_target('offline', 'offline')])
    wait(lambda: status(a.case + '-offline', 'offline', 'Retrying'), 'offline export retry independent of publication')
    podname = a.case + '-offline-pull'
    call(['create', '-f', '-'], dict(apiVersion='v1', kind='Pod', metadata=dict(name=podname), spec=dict(nodeSelector={'kubernetes.io/hostname': 'kimage-clean-2'}, automountServiceAccountToken=False, containers=[dict(name='app', image='platform://kimage-app/' + a.case + '-offline:1.0.0', imagePullPolicy='IfNotPresent')])))
    pod = wait(lambda: (lambda v: v if ready(v) else None)(get('pod', podname)), 'internal normal pull while external registry is down')
    assert pod['spec']['containers'][0]['image'].endswith('@' + offline_release['spec']['root']['digest'])
    result['externalDowntimeDoesNotGateBuildPublicationOrNormalPull'] = True
finally:
    restore(target_template, metadata['namespace'])
_, recovered = wait(lambda: status(a.case + '-offline', 'offline'), 'external recovery retry')
assert recovered['releaseUid'] == offline_release['metadata']['uid'] and get('imagerelease', offline_release['metadata']['name'])['spec'] == offline_release['spec']
result['destinationRecoveryRetriesWithoutRebuild'] = True

# Rotate the target's actual auth identity before supplying its new credential.
rotated = secrets.token_hex(32).encode()
htpasswd = subprocess.check_output(['/usr/sbin/htpasswd', '-niB', 'publisher'], input=rotated + b'\n', stderr=subprocess.DEVNULL)
secret = get('secret', metadata['pod'] + '-credentials', metadata['namespace'])
patch = dict(metadata=dict(uid=secret['metadata']['uid'], resourceVersion=secret['metadata']['resourceVersion']), data=dict(htpasswd=base64.b64encode(htpasswd).decode()))
path = a.output / 'rotate-target-secret.json'
path.write_text(json.dumps(patch));path.chmod(0o600)
call(['patch', 'secret', metadata['pod'] + '-credentials', '--type=merge', '--patch-file', str(path)], ns=metadata['namespace'])
target_template = restart_template(metadata['pod'], metadata['namespace'])
restore(target_template, metadata['namespace'])
patch_exports(a.case, [export_target('rotated', 'rotated')])
wait(lambda: status(a.case, 'rotated', 'Retrying'), 'stale credentials fail without internal failure')
secret = get('secret', credential_name)
patch = dict(metadata=dict(uid=secret['metadata']['uid'], resourceVersion=secret['metadata']['resourceVersion']), data=credentials_body(rotated))
path = a.output / 'rotate-client-secret.json'
path.write_text(json.dumps(patch));path.chmod(0o600)
call(['patch', 'secret', credential_name, '--type=merge', '--patch-file', str(path)])
(a.target / 'credentials/publisher-password').write_bytes(rotated)
_, rotated_status = wait(lambda: status(a.case, 'rotated'), 'rotated credential retry')
assert rotated_status['releaseUid'] == release['metadata']['uid'] and rotated_status['remoteDigest'] == release['spec']['root']['digest']
result['credentialRotationRetriesSameRetainedRelease'] = True

patch_exports(a.case, [])
wait(lambda: not get('image', a.case).get('status', {}).get('exports'), 'target removal clears independent status')
verify_remote(release, 'removed-target-retained', 'rotated')
assert get('imagerelease', release['metadata']['name'])['spec'] == release['spec']
result.update(targetRemovalDoesNotDeleteRemoteContent=True, retainedReleaseAndExecutionUnchanged=True)
public = json.dumps(get('image', a.case)).encode()
for value in [password, rotated]:
    assert value not in public and value not in call(['logs', 'kimage-clean-exporter', '--tail=3000'], ns='kimage-system')
result['credentialsAbsentFromStatusAndExporterLogs'] = True
(a.output / 'evidence.json').write_text(json.dumps(result, indent=2) + '\n')
print('authenticated retained export, idempotence/conflict, restart/retry, downtime, rotation and removal passed', flush=True)

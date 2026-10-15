#!/usr/bin/env python3
"""Final-binary independent pending status and retained export after reinstall."""
import argparse
import json
import os
import pathlib
import subprocess
import time

p = argparse.ArgumentParser(description=__doc__)
for key in ['kubeconfig', 'target', 'installer', 'output']:
    p.add_argument('--' + key, type=pathlib.Path, required=True)
a = p.parse_args()
assert 'server: https://127.0.0.1:27643' in a.kubeconfig.read_text()
b = ['kubectl', '--kubeconfig', str(a.kubeconfig), '--request-timeout=45s']
env = dict(os.environ, KUBECONFIG=str(a.kubeconfig.resolve()))
target = json.loads((a.target / 'metadata.json').read_text())
name = 'kimage-export-final-pending'
image_name = 'kimage-export-a1'


def call(args, body=None, ns='kimage-app'):
    value = subprocess.run(b + ['-n', ns] + args, input=None if body is None else json.dumps(body).encode(), capture_output=True, timeout=100)
    if value.returncode:
        raise RuntimeError(value.stderr.decode()[:1500])
    return value.stdout


def get(kind, name, ns='kimage-app'):
    return json.loads(call(['get', kind, name, '-o', 'json'], ns=ns))


def wait(predicate, label, seconds=300):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        value = predicate()
        if value:
            return value
        time.sleep(1)
    raise RuntimeError(label + ' deadline')


def ready(value):
    return any(c['type'] == 'Ready' and c['status'] == 'True' for c in value.get('status', {}).get('conditions', []))


def export_definition(label, tag):
    return dict(name=label, registry=dict(repository=target['repository'] + '/kimage-export-a1', tags=[tag], credentialsSecretRef=dict(name='kimage-export-a1-credentials'), caSecretRef=dict(name='kimage-export-a1-ca'), overwritePolicy='Never'))


operator = get('pod', 'kimage-clean-operator', 'kimage-system')
template = dict(apiVersion='v1', kind='Pod', metadata={key: operator['metadata'][key] for key in ['name', 'labels', 'annotations'] if key in operator['metadata']}, spec=operator['spec'])
assert operator['metadata']['labels']['kimage.dev/installation'] == 'kimage-clean'
call(['delete', 'pod', 'kimage-clean-operator', '--grace-period=1', '--wait=true', '--timeout=90s'], ns='kimage-system')
result = {}
try:
    source = dict(ociArchive=dict(claimName='kimage-export-unprovisioned', path='input.oci.tar', producerHandoff=True, root=dict(mediaType='application/vnd.oci.image.manifest.v1+json', digest='sha256:' + '0' * 64, size=1)))
    call(['create', '-f', '-'], dict(apiVersion='kimage.dev/v1alpha1', kind='Image', metadata=dict(name=name), spec=dict(version='1.0.0', platform=dict(os='linux', architecture='amd64'), source=source, exports=[export_definition('waiting', 'never-published')])))
    def pending():
        value = get('image', name)
        return value if any(s['phase'] == 'PendingRelease' for s in value.get('status', {}).get('exports', [])) else None
    value = wait(pending, 'independent initial PendingRelease status')
    assert value['status']['observedGeneration'] == 0 and not ready(value)
    assert value['status']['exports'][0].get('releaseUid') is None
    call(['delete', 'image', name, '--wait=true', '--timeout=60s'])
    result['pendingStatusBeforeAnyPublicationAcknowledgment'] = True
    image = get('image', image_name)
    release = get('imagerelease', image['status']['releaseRef'])
    call(['patch', 'image', image_name, '--type=merge', '-p', json.dumps(dict(metadata=dict(uid=image['metadata']['uid'], resourceVersion=image['metadata']['resourceVersion']), spec=dict(exports=[export_definition('final', 'final-binary')])) )])
    def complete():
        value = get('image', image_name)
        return next((s for s in value.get('status', {}).get('exports', []) if s['name'] == 'final' and s['phase'] == 'Complete'), None)
    status = wait(complete, 'final-binary retained export')
    assert status['releaseUid'] == release['metadata']['uid'] and status['remoteDigest'] == release['spec']['root']['digest']
    rootfile = a.output.with_suffix('.root.json')
    rootfile.write_text(json.dumps(release['spec']['root']))
    subprocess.run([str(a.installer.resolve()), 'verify-remote', '--root', str(rootfile.resolve()), '--tag', 'final-binary', '--namespace', target['namespace'], '--pod', target['pod'], '--origin', target['origin'], '--repository', (target['repository'] + '/kimage-export-a1').split('/', 1)[1], '--ca', str((a.target / 'credentials/ca.crt').resolve()), '--password-file', str((a.target / 'credentials/publisher-password').resolve())], env=env, check=True, timeout=600, stdout=subprocess.DEVNULL)
    result.update(releaseUid=release['metadata']['uid'], root=release['spec']['root'], finalBinaryRetainedExportWithoutBuildController=True, independentFullRemoteGraphAndTagVerified=True)
    image = get('image', image_name)
    call(['patch', 'image', image_name, '--type=merge', '-p', json.dumps(dict(metadata=dict(uid=image['metadata']['uid'], resourceVersion=image['metadata']['resourceVersion']), spec=dict(exports=[])))])
    wait(lambda: not get('image', image_name).get('status', {}).get('exports'), 'final target removed')
finally:
    call(['delete', 'image', name, '--ignore-not-found=true', '--wait=true', '--timeout=60s'])
    call(['create', '-f', '-'], template, ns='kimage-system')
    wait(lambda: ready(get('pod', 'kimage-clean-operator', 'kimage-system')), 'publication controller restored')
result['publicationControllerRestored'] = True
a.output.write_text(json.dumps(result, indent=2) + '\n')
print('final binary independently reports PendingRelease and exports retained bytes with verified remote tag', flush=True)

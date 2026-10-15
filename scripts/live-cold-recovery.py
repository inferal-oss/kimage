#!/usr/bin/env python3
"""Resumable cold recovery/maintenance rotation on the owned isolated VM only.

The external Linux installer has no node mounts or runtime socket. Cache eviction
belongs to the fixture provider, and is restricted to this installation's images.
Private receipts allow continuation after an interrupted native phase.
"""
import argparse
import hashlib
import json
import os
import pathlib
import re
import shutil
import subprocess

p = argparse.ArgumentParser(description=__doc__)
for name in ['kubeconfig', 'options', 'bundle', 'fixture', 'output']:
    p.add_argument('--' + name, type=pathlib.Path, required=True)
p.add_argument('--trusted-inventory', required=True)
a = p.parse_args()
root = pathlib.Path(__file__).resolve().parent.parent
assert re.search(r'server: https://127\.0\.0\.1:27643\s', a.kubeconfig.read_text())
o = json.loads(a.options.read_text())
assert (o['namespace'], o['applicationNamespace'], o['installation'], o['runtimeProfile']) == ('kimage-system', 'kimage-app', 'kimage-clean', 'k3s')
inventory = json.loads((a.bundle / 'inventory.json').read_bytes())
assert 'sha256:' + hashlib.sha256((a.bundle / 'inventory.json').read_bytes()).hexdigest() == a.trusted_inventory
assert set(inventory['components']) == {'registry', 'gateway', 'proxy', 'operator', 'admission', 'input', 'builder'}
a.output.mkdir(parents=True, exist_ok=True)
os.chmod(a.output, 0o700)
k = ['kubectl', '--kubeconfig', str(a.kubeconfig), '--request-timeout=20s']

def call(cmd, body=None, timeout=120):
    result = subprocess.run([str(v) for v in cmd], input=None if body is None else json.dumps(body).encode(), capture_output=True, timeout=timeout)
    if result.returncode:
        raise RuntimeError('command failed: ' + str(cmd[0]) + ': ' + result.stderr.decode()[:1500])
    return result.stdout

def get(kind, name=None, namespace=None):
    return json.loads(call(k + ([] if namespace is None else ['-n', namespace]) + ['get', kind] + ([] if name is None else [name]) + ['-o', 'json']))

def provider(operation, images=None):
    cmd = ['python3', root / 'scripts/isolated-fixture.py', 'request', '--directory', a.fixture, '--operation', operation]
    if images is not None:
        path = a.output / 'evict.json'
        path.write_text(json.dumps(images))
        cmd += ['--images', path]
    return json.loads(call(cmd, timeout=150))

def save(name, value):
    path = a.output / name
    path.write_text(json.dumps(value, indent=2) + '\n')
    path.chmod(0o600)

registry = a.output / 'registry-credentials'
admission = a.output / 'admission-credentials'
builder = a.output / 'builder-credentials'
if not registry.exists():
    call(['python3', root / 'scripts/prepare-test-credentials.py', '--dir', registry, '--registry-dns', 'kimage-clean-registry.kimage-system.svc'])
if not admission.exists():
    call(['python3', root / 'scripts/prepare-test-credentials.py', '--dir', admission, '--registry-dns', 'kimage-clean-admission.kimage-system.svc'])
    shutil.copyfile(root / '.local/isolated/admission-credentials/resolution-key', admission / 'resolution-key')
if not builder.exists():
    call(['python3', root / 'scripts/prepare-builder-test-credentials.py', '--dir', builder, '--dns', 'kimage-clean-builder.kimage-system.svc'])
new_options = dict(o, credentials=str(registry.resolve()))
save('options.json', new_options)

def container_path(path):
    return '/work/' + str(pathlib.Path(path).resolve().relative_to(root))

def install(phase):
    options = dict(new_options, credentials=container_path(registry))
    cmd = ['docker', 'run', '--rm', '--pull=never', '--name', 'kimage-linux-cold-recovery', '--label', 'kimage.fixture=kimage-clean', '--platform', 'linux/amd64', '--network', 'host', '--read-only', '--user', str(os.getuid()) + ':' + str(os.getgid()), '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges', '--memory', '512m', '--cpus', '1', '--tmpfs', '/tmp:rw,nosuid,nodev,size=512m,mode=1777', '--mount', 'type=bind,src=' + str(root) + ',dst=/work,readonly', '--workdir', '/work', '-e', 'KUBECONFIG=' + container_path(a.kubeconfig), '--entrypoint', container_path(a.bundle / 'kimage-install-linux-amd64'), 'b36c246742b4', phase]
    for key, value in options.items():
        cmd += ['--' + ''.join('-' + c.lower() if c.isupper() else c for c in key), str(value)]
    if phase != 'uninstall':
        cmd += ['--bundle', container_path(a.bundle), '--trusted-inventory', a.trusted_inventory]
    if phase == 'control-plane':
        cmd += ['--admission-credentials', container_path(admission)]
    if phase == 'builder':
        cmd += ['--builder-credentials', container_path(builder)]
    # Full logs stay local. Errors from the native installer omit credentials.
    log = a.output / (phase + '.log')
    with log.open('wb') as output:
        result = subprocess.run(cmd, stdout=output, stderr=subprocess.STDOUT, timeout=1500)
    log.chmod(0o600)
    assert result.returncode == 0, phase + ' failed; inspect private local phase log'
    print('external Linux native ' + phase + ' passed', flush=True)

baseline_path = a.output / 'baseline.json'
if not baseline_path.exists():
    releases = get('imagereleases', namespace=o['applicationNamespace'])['items']
    save('baseline.json', dict(pvcUid=get('pvc', 'kimage-clean-data', o['namespace'])['metadata']['uid'], releases=[dict(name=r['metadata']['name'], uid=r['metadata']['uid'], spec=r['spec']) for r in releases if r.get('status', {}).get('verified')], crds=[dict(name=n, uid=get('crd', n)['metadata']['uid'], spec=get('crd', n)['spec']) for n in ['images.kimage.dev', 'imagereleases.kimage.dev']]))
baseline = json.loads(baseline_path.read_text())
checkpoint = a.output / 'cold.json'
if not checkpoint.exists():
    install('uninstall')
    images = provider('images')['images']
    refs = sorted({ref for image in images for ref in image.get('repoDigests', []) + image.get('repoTags', []) if re.fullmatch(r'127\.0\.0\.1:17641/kimage-system/system-[a-z]+@sha256:[0-9a-f]{64}', ref)})
    for start in range(0, len(refs), 16):
        provider('evict_system_images', refs[start:start + 16])
    remaining = {ref for image in provider('images')['images'] for ref in image.get('repoDigests', []) + image.get('repoTags', [])}
    target = ['127.0.0.1:17641/kimage-system/system-' + role + '@' + value['root']['digest'] for role, value in inventory['components'].items()]
    assert not remaining.intersection(refs + target), 'system image remained cached'
    assert not provider('external_probe')['externalReachable'], 'fixture unexpectedly has external access'
    save('cold.json', dict(evictedReferences=refs, allSevenTargetReferencesAbsent=target, externalNetworkBlocked=True))
    print('all seven component references absent; external node access blocked', flush=True)
for phase in ['recover', 'seed', 'handover', 'control-plane', 'builder']:
    receipt = a.output / (phase + '.passed')
    if not receipt.exists():
        install(phase)
        receipt.write_text(a.trusted_inventory + '\n')
    else:
        assert receipt.read_text().strip() == a.trusted_inventory
assert get('pvc', 'kimage-clean-data', o['namespace'])['metadata']['uid'] == baseline['pvcUid']
for old in baseline['releases']:
    current = get('imagerelease', old['name'], o['applicationNamespace'])
    assert current['metadata']['uid'] == old['uid'] and current['spec'] == old['spec'] and current['status']['verified']
for old in baseline['crds']:
    current = get('crd', old['name'])
    assert current['metadata']['uid'] == old['uid'] and current['spec'] == old['spec']
save('results.json', dict(trustedInventory=a.trusted_inventory, externalUnprivilegedLinuxNativeInstaller=True, allSevenComponentReferencesAbsentBeforeRecovery=True, nodeExternalNetworkBlocked=True, samePvcUid=baseline['pvcUid'], retainedVerifiedReleasesPreserved=len(baseline['releases']), crdUidsAndSchemasPreserved=True, newRegistryAndAdmissionTrustRoots=True, newIndependentRegistryPasswords=True, allNativePhasesPassed=True, credentialRejectionAndFreshPullChecksPending=True))
print('cold Linux native recovery preserved PVC, releases and schemas; rotation probes remain', flush=True)

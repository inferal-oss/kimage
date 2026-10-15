#!/usr/bin/env python3
"""Owned-fixture maintenance recovery; preserves retained data and CRD identities.

This development deployment is deliberately recorded separately from product
self-upgrade acceptance. Every native phase remains resumable and API-only.
"""
import argparse
import hashlib
import json
import os
import pathlib
import subprocess

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--evict-system-images', action='store_true', help='owned QEMU fixtures only: evict component CRI caches after uninstall')
p.add_argument('--join-node', help='restore this additional node after the control plane')
for key in ['kubeconfig', 'options', 'bundle', 'installer', 'admission-credentials', 'builder-credentials', 'output']:
    p.add_argument('--' + key, type=pathlib.Path, required=True)
a = p.parse_args()
assert 'server: https://127.0.0.1:27643' in a.kubeconfig.read_text()
options = json.loads(a.options.read_text())
assert options['installation'] == 'kimage-clean' and options['namespace'] == 'kimage-system'
a.output.mkdir(parents=True, exist_ok=True)
env = dict(os.environ, KUBECONFIG=str(a.kubeconfig.resolve()))
env.pop('KIMAGE_API_SERVER', None)
env.pop('KIMAGE_TLS_SERVER_NAME', None)
trusted = 'sha256:' + hashlib.sha256((a.bundle / 'inventory.json').read_bytes()).hexdigest()


def objects(kind, namespace=None):
    args = ['kubectl', '--kubeconfig', str(a.kubeconfig), '--request-timeout=45s']
    if namespace:
        args += ['-n', namespace]
    v = json.loads(subprocess.check_output(args + ['get', kind, '-o', 'json'], env=env, timeout=60))
    return {o['metadata']['name']: dict(uid=o['metadata']['uid'], spec=o['spec']) for o in v['items']}


baseline_path = a.output / 'baseline.json'
if baseline_path.exists():
    baseline = json.loads(baseline_path.read_text())
    assert baseline['inventory'] == trusted
else:
    baseline = dict(inventory=trusted, pvc=objects('pvc', 'kimage-system'),
                    releases=objects('imagereleases', 'kimage-app'),
                    schemas=objects('crd'))
    baseline_path.write_text(json.dumps(baseline, indent=2) + '\n')

for phase in ['uninstall', 'recover', 'seed', 'handover', 'control-plane', 'builder']:
    done = a.output / (phase + '.done')
    if done.exists():
        continue
    args = [str(a.installer.resolve()), phase]
    for key, value in options.items():
        args += ['--' + ''.join('-' + c.lower() if c.isupper() else c for c in key), str(value)]
    if phase != 'uninstall':
        args += ['--bundle', str(a.bundle.resolve()), '--trusted-inventory', trusted]
    if phase == 'control-plane':
        args += ['--admission-credentials', str(a.admission_credentials.resolve())]
    if phase == 'builder':
        args += ['--builder-credentials', str(a.builder_credentials.resolve())]
    log = a.output / (phase + '.log')
    with log.open('wb') as stream:
        result = subprocess.run(args, env=env, stdout=stream, stderr=subprocess.STDOUT, timeout=1500)
    log.chmod(0o600)
    assert result.returncode == 0, phase + ' failed; inspect private phase log'
    if phase == 'uninstall' and a.evict_system_images:
        import re
        pattern = re.compile(r'127\.0\.0\.1:17641/kimage-system/system-(registry|gateway|proxy|operator|admission|input|builder|exporter)@sha256:[0-9a-f]{64}')
        for fixture in ['qemu-routed', 'qemu-second']:
            provider = ['python3', 'scripts/isolated-fixture.py', 'request', '--directory', '.local/' + fixture]
            before = json.loads(subprocess.check_output(provider + ['--operation', 'images'], timeout=130))
            # One exact reference per CRI image ID avoids duplicate removals of
            # manifests sharing the same config/image ID.
            refs = [next(ref for ref in item.get('repoDigests', []) if pattern.fullmatch(ref)) for item in before['images'] if any(pattern.fullmatch(ref) for ref in item.get('repoDigests', []))]
            for start in range(0, len(refs), 16):
                path = a.output / (fixture + '-evict.json')
                path.write_text(json.dumps(refs[start:start + 16]))
                removed = json.loads(subprocess.check_output(provider + ['--operation', 'evict_system_images', '--images', str(path)], timeout=130))
                assert removed['code'] == 0, 'fixture cache eviction failed'
            after = json.loads(subprocess.check_output(provider + ['--operation', 'images'], timeout=130))
            assert not any(pattern.fullmatch(ref) for item in after['images'] for ref in item.get('repoDigests', []))
        (a.output / 'both-node-component-caches-absent.done').write_text('passed\n')
    done.write_text('passed\n')
    print('owned maintenance phase', phase, 'passed', flush=True)

if a.join_node:
    args = [str(a.installer.resolve()), 'join-node']
    for key, value in options.items():
        args += ['--' + ''.join('-' + c.lower() if c.isupper() else c for c in key), str(value)]
    args += ['--bundle', str(a.bundle.resolve()), '--trusted-inventory', trusted, '--target-node', a.join_node, '--recover']
    subprocess.run(args, env=env, check=True, timeout=900)

assert objects('pvc', 'kimage-system') == baseline['pvc']
assert objects('imagereleases', 'kimage-app') == baseline['releases']
schemas = objects('crd')
for name in ['images.kimage.dev', 'imagereleases.kimage.dev']:
    assert schemas[name]['uid'] == baseline['schemas'][name]['uid']
result = dict(inventory=trusted, sameRegistryPvc=True, retainedReleaseUidsAndSpecsPreserved=True,
              schemaUidsPreserved=True, deploymentMethod='API-only maintenance uninstall/recover; not self-upgrade')
if a.evict_system_images:
    assert (a.output / 'both-node-component-caches-absent.done').exists()
    result['bothNodeComponentCachesAbsentBeforeRecovery'] = True
if a.join_node:
    result['additionalNodeRestoredThroughNativeJoin'] = a.join_node
(a.output / 'evidence.json').write_text(json.dumps(result, indent=2) + '\n')
print('retained data and schema identities preserved across maintenance recovery', flush=True)

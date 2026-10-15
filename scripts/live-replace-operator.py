#!/usr/bin/env python3
"""Deploy an operator/admission component through ordinary API/pull paths on the fixture.

This is a test harness deployment, not product self-upgrade acceptance.
"""
import argparse
import hashlib
import json
import os
import pathlib
import subprocess
import time

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--role', choices=['operator','admission'], default='operator')
for key in ['kubeconfig', 'bundle', 'credentials', 'installer', 'output']:
    p.add_argument('--' + key, type=pathlib.Path, required=True)
a = p.parse_args()
assert 'server: https://127.0.0.1:27643' in a.kubeconfig.read_text()
a.output.mkdir(parents=True, exist_ok=True)
inventory = json.loads((a.bundle / 'inventory.json').read_text())
role = a.role
root = inventory['components'][role]['root']
rootfile = a.output / 'root.json'
rootfile.write_text(json.dumps(root))
env = dict(os.environ, KUBECONFIG=str(a.kubeconfig.resolve()))
env.pop('KIMAGE_API_SERVER', None)
env.pop('KIMAGE_TLS_SERVER_NAME', None)
subprocess.run([str(a.installer.resolve()), 'publish', '--archive', str((a.bundle / (role + '.oci.tar')).resolve()),
                '--root', str(rootfile.resolve()), '--namespace', 'kimage-system', '--pod', 'kimage-clean-registry',
                '--origin', 'https://kimage-clean-registry.kimage-system.svc:5443/', '--repository', 'kimage-system/system-' + role,
                '--ca', str((a.credentials / 'ca.crt').resolve()), '--password-file', str((a.credentials / 'publisher-password').resolve())],
               env=env, check=True, timeout=240)
b = ['kubectl', '--kubeconfig', str(a.kubeconfig), '--request-timeout=45s', '-n', 'kimage-system']
old = json.loads(subprocess.check_output(b + ['get', 'pod', 'kimage-clean-' + role, '-o', 'json'], timeout=60))
assert old['metadata']['labels']['kimage.dev/installation'] == 'kimage-clean'
old_uid = old['metadata']['uid']
replacement = dict(apiVersion='v1', kind='Pod', metadata=dict(name='kimage-clean-' + role, labels=old['metadata']['labels']), spec=old['spec'])
assert len(replacement['spec']['containers']) == 1
replacement['spec']['containers'][0]['image'] = '127.0.0.1:17641/kimage-system/system-' + role + '@' + root['digest']
subprocess.run(b + ['delete', 'pod', 'kimage-clean-' + role, '--wait=true', '--timeout=90s'], check=True, timeout=100)
subprocess.run(b + ['create', '-f', '-'], input=json.dumps(replacement).encode(), check=True, timeout=60)
end = time.monotonic() + 240
while time.monotonic() < end:
    pod = json.loads(subprocess.check_output(b + ['get', 'pod', 'kimage-clean-' + role, '-o', 'json'], timeout=60))
    if any(c['type'] == 'Ready' and c['status'] == 'True' for c in pod.get('status', {}).get('conditions', [])):
        break
    time.sleep(2)
else:
    raise RuntimeError('development operator readiness deadline')
(a.output / 'evidence.json').write_text(json.dumps(dict(oldPodUid=old_uid, podUid=pod['metadata']['uid'],
    component=role, componentRoot=root['digest'], bundleInventory='sha256:' + hashlib.sha256((a.bundle / 'inventory.json').read_bytes()).hexdigest(),
    deploymentMethod='ordinary API development Pod replacement; not product self-upgrade'), indent=2) + '\n')
print('updated development ' + role + ' ready through normal image pull', flush=True)

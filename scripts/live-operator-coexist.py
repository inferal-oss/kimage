#!/usr/bin/env python3
"""Old/new schema-compatible controllers coexist, restart and retain CRD data."""
import argparse
import base64
import hashlib
import json
import os
import pathlib
import subprocess
import time

p = argparse.ArgumentParser(description=__doc__)
for key in ['kubeconfig', 'bundle', 'credentials', 'installer', 'probe', 'output']:
    p.add_argument('--' + key, type=pathlib.Path, required=True)
p.add_argument('--case', required=True)
a = p.parse_args()
assert 'server: https://127.0.0.1:27643' in a.kubeconfig.read_text()
assert a.case.startswith('kimage-coexist-') and a.case.replace('-', '').isalnum()
a.output.mkdir(parents=True, exist_ok=True)
b = ['kubectl', '--kubeconfig', str(a.kubeconfig), '--request-timeout=45s']
env = dict(os.environ, KUBECONFIG=str(a.kubeconfig.resolve()))


def call(args, body=None, ns='kimage-system'):
    value = subprocess.run(b + ['-n', ns] + args, input=None if body is None else json.dumps(body).encode(), capture_output=True, timeout=100)
    if value.returncode:
        raise RuntimeError(value.stderr.decode()[:1800])
    return value.stdout


def get(kind, name, ns='kimage-system'):
    return json.loads(call(['get', kind, name, '-o', 'json'], ns=ns))


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


def schemas():
    return {name: dict(uid=(v := get('crd', name))['metadata']['uid'], spec=v['spec']) for name in ['images.kimage.dev', 'imagereleases.kimage.dev']}


baseline = schemas()
old = get('pod', 'kimage-clean-operator')
root = json.loads((a.bundle / 'inventory.json').read_text())['components']['operator']['root']
assert not old['spec']['containers'][0]['image'].endswith('@' + root['digest'])
rootfile = a.output / 'operator-root.json'
rootfile.write_text(json.dumps(root))
subprocess.run([str(a.installer.resolve()), 'publish', '--archive', str((a.bundle / 'operator.oci.tar').resolve()), '--root', str(rootfile.resolve()), '--namespace', 'kimage-system', '--pod', 'kimage-clean-registry', '--origin', 'https://kimage-clean-registry.kimage-system.svc:5443/', '--repository', 'kimage-system/system-operator', '--ca', str((a.credentials / 'ca.crt').resolve()), '--password-file', str((a.credentials / 'publisher-password').resolve())], env=env, check=True, timeout=300)
new = dict(apiVersion='v1', kind='Pod', metadata=dict(name='kimage-clean-operator-next', labels=old['metadata']['labels']), spec=old['spec'])
new['spec']['containers'][0]['image'] = '127.0.0.1:17641/kimage-system/system-operator@' + root['digest']
call(['create', '-f', '-'], new)
try:
    second = wait(lambda: (lambda v: v if ready(v) else None)(get('pod', 'kimage-clean-operator-next')), 'new operator ready')
    assert ready(get('pod', 'kimage-clean-operator')) and schemas() == baseline
    source = a.case + '-context'
    call(['create', '-f', '-'], dict(apiVersion='v1', kind='ConfigMap', metadata=dict(name=source), data=dict(message=a.case), binaryData=dict(probe=base64.b64encode(a.probe.read_bytes()).decode())), ns='kimage-app')
    build = dict(format='Containerfile', definition=dict(inline='FROM scratch\nCOPY probe /probe\nCOPY message /message\nENTRYPOINT ["/probe"]\n'), execution=dict(strategy='Direct', builderRef='default'), inputs=dict(contextPath='/workspace/context', volumes=[dict(name='context', configMap=dict(name=source, items=[dict(key='probe', path='probe', mode=365), dict(key='message', path='message', mode=292)]))], volumeMounts=[dict(name='context', mountPath='/workspace/context', readOnly=True)]))
    call(['create', '-f', '-'], dict(apiVersion='kimage.dev/v1alpha1', kind='Image', metadata=dict(name=a.case), spec=dict(version='1.0.0', platform=dict(os='linux', architecture='amd64'), source=dict(build=build))), ns='kimage-app')
    image = wait(lambda: (lambda v: v if ready(v) else None)(get('image', a.case, 'kimage-app')), 'coexisting controllers publication')
    release = get('imagerelease', image['status']['releaseRef'], 'kimage-app')
    assert release['status']['verified'] and schemas() == baseline
    old_template = dict(apiVersion='v1', kind='Pod', metadata=dict(name='kimage-clean-operator', labels=old['metadata']['labels']), spec=old['spec'])
    call(['delete', 'pod', 'kimage-clean-operator', '--wait=true', '--timeout=90s'])
    call(['create', '-f', '-'], old_template)
    restarted = wait(lambda: (lambda v: v if ready(v) else None)(get('pod', 'kimage-clean-operator')), 'old compatible controller restarted')
    assert restarted['metadata']['uid'] != old['metadata']['uid'] and ready(get('pod', 'kimage-clean-operator-next'))
    assert get('imagerelease', release['metadata']['name'], 'kimage-app')['spec'] == release['spec'] and schemas() == baseline
    result = dict(oldOperatorImage=old['spec']['containers'][0]['image'], newOperatorImage=new['spec']['containers'][0]['image'], oldAndNewReadyTogether=True, publishedWithBothControllers=True, oldCompatibleBinaryRestarted=True, schemaUidsAndSpecsPreserved=True, releaseUid=release['metadata']['uid'], root=release['spec']['root'], binaryCompatibility='same v1alpha1 schema revision 3; no storage-version migration')
    (a.output / 'evidence.json').write_text(json.dumps(result, indent=2) + '\n')
finally:
    call(['delete', 'pod', 'kimage-clean-operator-next', '--ignore-not-found=true', '--wait=true', '--timeout=90s'])
print('old/new compatible operator coexistence, publication and restart passed', flush=True)

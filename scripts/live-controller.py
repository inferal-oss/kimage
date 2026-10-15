#!/usr/bin/env python3
"""Real disconnected fixture controller/admission proof; never targets a shared cluster."""
import argparse
import base64
import json
import os
import pathlib
import re
import subprocess
import time

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--kubeconfig', type=pathlib.Path, required=True)
p.add_argument('--options', type=pathlib.Path, required=True)
p.add_argument('--bundle', type=pathlib.Path, required=True)
p.add_argument('--trusted-inventory', required=True)
p.add_argument('--installer', type=pathlib.Path, required=True)
p.add_argument('--admission-credentials', type=pathlib.Path, required=True)
p.add_argument('--producer', type=pathlib.Path, required=True)
p.add_argument('--archive', type=pathlib.Path, required=True)
p.add_argument('--results', type=pathlib.Path, required=True)
p.add_argument('--resume-controller', action='store_true', help='resume after the fixture producer was successfully terminated')
p.add_argument('--case', default='', help='unique suffix for a fresh acceptance run')
p.add_argument('--check-writer', action='store_true', help='prove capture waits for the live writable producer')
p.add_argument('--fixture-probe', type=pathlib.Path, help='bounded fixture helper on an already cached carrier when admission is active')
a = p.parse_args()
assert re.search(r'server: https://127\.0\.0\.1:[0-9]+\s', a.kubeconfig.read_text()), 'isolated loopback fixture required'
o = json.loads(a.options.read_text())
system, app, install = o['namespace'], o['applicationNamespace'], o['installation']
assert system.startswith('kimage-') and app.startswith('kimage-') and system != app
assert o['runtimeProfile'] == 'k3s'
assert not a.case or re.fullmatch('[a-z0-9-]{1,16}', a.case)
case = install + ('-' + a.case if a.case else '')
env = os.environ.copy()
env['KUBECONFIG'] = str(a.kubeconfig.resolve())
for key in ['KIMAGE_API_SERVER', 'KIMAGE_TLS_SERVER_NAME']:
    env.pop(key, None)
base = ['kubectl', '--kubeconfig', str(a.kubeconfig), '--request-timeout=10s']

def kubectl(args, body=None, success=True):
    r = subprocess.run(base + args, input=None if body is None else json.dumps(body).encode(), env=env, capture_output=True)
    if success and r.returncode:
        raise RuntimeError(r.stderr.decode()[:2000])
    return r

def get(kind, name, ns=app):
    return json.loads(kubectl(['-n', ns, 'get', kind, name, '-o', 'json']).stdout)

def create(value, ns=app, dry=False, success=True):
    args = ['-n', ns, 'create', '-f', '-', '-o', 'json']
    if dry:
        args.append('--dry-run=server')
    return kubectl(args, value, success)

def wait(predicate, label, timeout=240):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(1)
    raise RuntimeError(label + ' timeout')

def ready(kind, name, ns=app):
    v = get(kind, name, ns)
    return v if any(c['type'] == 'Ready' and c['status'] == 'True' for c in v.get('status', {}).get('conditions', [])) else None

def installer(phase):
    args = []
    for key, value in o.items():
        flag = ''.join('-' + c.lower() if c.isupper() else c for c in key)
        args += ['--' + flag, str(value)]
    cmd = [str(a.installer.resolve()), phase] + args + ['--bundle', str(a.bundle.resolve()), '--trusted-inventory', a.trusted_inventory]
    if phase == 'control-plane':
        cmd += ['--admission-credentials', str(a.admission_credentials.resolve())]
    subprocess.run(cmd, env=env, check=True)

producer_root = json.loads((a.producer / 'probe.root.json').read_text())
root = json.loads((a.archive / 'probe.root.json').read_text())
image_name = case + '-app'
image_spec = dict(apiVersion='kimage.dev/v1alpha1', kind='Image', metadata=dict(name=image_name), spec=dict(version='1.0.0', channels=dict(stable='1.0.0'), platform=dict(os='linux', architecture='amd64'), source=dict(ociArchive=dict(claimName=case + '-source', path='input.oci.tar', root=root, producerHandoff=True))))
assert producer_root['digest'] != root['digest'], 'producer and new output must have distinct roots'
producer_ref = f"127.0.0.1:{o['port']}/{system}/system-producer@{producer_root['digest']}"
source = case + '-source'
producer = case + '-producer'
if not a.resume_controller:
    if not a.fixture_probe:
        subprocess.run([str(a.installer.resolve()), 'publish', '--archive', str((a.producer / 'probe.oci.tar').resolve()), '--root', str((a.producer / 'probe.root.json').resolve()), '--namespace', system, '--pod', install + '-registry', '--origin', f'https://{install}-registry.{system}.svc:5443/', '--repository', system + '/system-producer', '--ca', str(pathlib.Path(o['credentials']) / 'ca.crt'), '--password-file', str(pathlib.Path(o['credentials']) / 'publisher-password')], env=env, check=True)
    source = case + '-source'
    create(dict(apiVersion='v1', kind='PersistentVolumeClaim', metadata=dict(name=source), spec=dict(accessModes=['ReadWriteOnce'], storageClassName=o['storageClass'], resources=dict(requests=dict(storage='1Gi')))))
    producer = case + '-producer'
    producer_pod = dict(apiVersion='v1', kind='Pod', metadata=dict(name=producer), spec=dict(automountServiceAccountToken=False, restartPolicy='Never', affinity=dict(nodeAffinity=dict(requiredDuringSchedulingIgnoredDuringExecution=dict(nodeSelectorTerms=[dict(matchFields=[dict(key='metadata.name', operator='In', values=[o['node']])])]))), containers=[dict(name='producer', image=producer_ref, imagePullPolicy='IfNotPresent', securityContext=dict(runAsUser=0, runAsGroup=0, allowPrivilegeEscalation=False, readOnlyRootFilesystem=True, capabilities=dict(drop=['ALL'])), resources=dict(requests=dict(cpu='10m', memory='16Mi'), limits=dict(cpu='500m', memory='64Mi')), volumeMounts=[dict(name='source', mountPath='/source')])], volumes=[dict(name='source', persistentVolumeClaim=dict(claimName=source))]))
    executable = '/kimage-probe'
    if a.fixture_probe:
        helper = a.fixture_probe.read_bytes(); assert len(helper) <= 512 * 1024
        create(dict(apiVersion='v1', kind='ConfigMap', metadata=dict(name=case + '-helper'), immutable=True, binaryData=dict(probe=base64.b64encode(helper).decode())))
        executable = '/helper/probe'
        producer_pod['spec']['containers'][0].update(image=o['carrier'], imagePullPolicy='Never', command=[executable])
        producer_pod['spec']['containers'][0]['volumeMounts'].append(dict(name='helper', mountPath='/helper', readOnly=True))
        producer_pod['spec']['volumes'].append(dict(name='helper', configMap=dict(name=case + '-helper', defaultMode=365)))
    create(producer_pod)
    wait(lambda: ready('pod', producer), 'producer')
    with (a.archive / 'probe.oci.tar').open('rb') as stdin:
        subprocess.run(base + ['-n', app, 'exec', '-i', producer, '--', executable, '--stage-fixture'], stdin=stdin, env=env, check=True)
    if a.check_writer:
        installer('control-plane')
        create(image_spec)
        def writer_deferred():
            current = get('image', image_name)
            assert not any(c['type'] == 'Ready' and c['status'] == 'True' for c in current.get('status', {}).get('conditions', []))
            return current if any(c['type'] == 'Ready' and c['reason'] == 'PvcWriterNotQuiescent' for c in current.get('status', {}).get('conditions', [])) else None
        deferred = wait(writer_deferred, 'known writable producer refusal')
        assert not any(r['spec']['imageUid'] == deferred['metadata']['uid'] and r.get('status', {}).get('verified') for r in json.loads(kubectl(['-n', app, 'get', 'imagereleases', '-o', 'json']).stdout)['items'])
        print('capture deferred while the known writable producer is present', flush=True)
    kubectl(['-n', app, 'delete', 'pod', producer, '--wait=true', '--timeout=90s'])
    print('PVC input persisted; producer terminated before capture', flush=True)
else:
    assert get('pvc', source)['status']['phase'] == 'Bound'
    assert kubectl(['-n', app, 'get', 'pod', producer], success=False).returncode
installer('control-plane')
installer('control-plane')
existing = kubectl(['-n', app, 'get', 'image', image_name, '-o', 'json'], success=False)
if existing.returncode:
    create(image_spec)
else:
    current = json.loads(existing.stdout)
    assert (a.resume_controller or a.check_writer) and current['spec']['version'] == '1.0.0'
    assert all(current['spec']['source']['ociArchive']['root'][k] == root[k] for k in ['digest', 'mediaType', 'size'])
image = wait(lambda: ready('image', image_name), 'published Image')
release = get('imagerelease', image['status']['releaseRef'])
assert release['status']['verified']
assert all(release['spec']['root'][k] == root[k] for k in ['digest', 'mediaType', 'size'])
assert not release['metadata'].get('ownerReferences'), 'release must survive Image deletion'
reference = f"127.0.0.1:{o['port']}/{app}/{image['metadata']['uid']}@{root['digest']}"
uri = f'platform://{app}/{image_name}:stable'
pod = dict(apiVersion='v1', kind='Pod', metadata=dict(name=case + '-managed'), spec=dict(automountServiceAccountToken=False, containers=[dict(name='app', image=uri, imagePullPolicy='Always', resources=dict(requests=dict(cpu='10m', memory='16Mi'), limits=dict(cpu='500m', memory='64Mi'))), dict(name='unmanaged', image=o['carrier'], imagePullPolicy='Never', resources=dict(requests=dict(cpu='10m', memory='8Mi'), limits=dict(cpu='100m', memory='32Mi')))], initContainers=[dict(name='sidecar', image=uri, restartPolicy='Always', resources=dict(requests=dict(cpu='10m', memory='16Mi'), limits=dict(cpu='100m', memory='32Mi')))]))
before = get('imagerelease', release['metadata']['name'])['metadata']['resourceVersion']
dry = json.loads(create(pod, dry=True).stdout)
assert dry['spec']['containers'][0]['image'] == reference
assert dry['spec']['containers'][0]['imagePullPolicy'] == 'IfNotPresent'
assert dry['spec']['containers'][1]['image'] == o['carrier'] and dry['spec']['containers'][1]['imagePullPolicy'] == 'Never'
assert dry['spec']['initContainers'][0]['image'] == reference
assert get('imagerelease', release['metadata']['name'])['metadata']['resourceVersion'] == before
assert kubectl(['-n', app, 'get', 'pod', pod['metadata']['name']], success=False).returncode
aliases = [reference.replace('127.0.0.1:', alias + ':', 1) for alias in ['localhost', 'LOCALHOST.', '127.0.0.2', '[::ffff:127.0.0.1]']]
for invalid in [f'platform://{app}/{image_name}:9.9.9', f'platform://{system}/{image_name}:1.0.0', reference] + aliases:
    denied = json.loads(json.dumps(pod))
    denied['spec']['containers'][0]['image'] = invalid
    assert create(denied, dry=True, success=False).returncode, 'unexpected admission success'
create(pod)
stored = wait(lambda: ready('pod', pod['metadata']['name']), 'managed Pod')
assert stored['spec']['containers'][0]['image'] == reference
logs = kubectl(['-n', system, 'logs', install + '-proxy', '--tail=300']).stdout.decode()
assert f"/v2/{app}/{image['metadata']['uid']}/manifests/{root['digest']}" in logs, 'missing ordinary runtime root request'
print('new Image → verified release → resolved regular/init-sidecar Pod Running', flush=True)
result = dict(root=root['digest'], imageUid=image['metadata']['uid'], releaseUid=release['metadata']['uid'], sourceRemoved=False, producerHandoff=True, knownWriterDeferred=a.check_writer, operatorCreatedSchemas=True, controlPlaneIdempotent=True, dryRunReadOnly=True, regularAndInitSidecarRunning=True, unmanagedPreserved=True, rawInternalAndCrossNamespaceDenied=True, loopbackAliasesDenied=True, runtimeRootRequestObserved=True, retainedAfterImageDeletion=False)
a.results.write_text(json.dumps(result, indent=2) + '\n')
kubectl(['-n', app, 'delete', 'pvc', source, '--wait=true', '--timeout=90s'])
kubectl(['-n', app, 'delete', 'image', image_name, '--wait=true', '--timeout=90s'])
retained = json.loads(json.dumps(pod));retained['metadata']['name'] = case + '-retained';retained['spec']['containers'][0]['image'] = f'platform://{app}/{image_name}:1.0.0';retained['spec']['initContainers'][0]['image'] = f'platform://{app}/{image_name}:1.0.0'
create(retained)
wait(lambda: ready('pod', retained['metadata']['name']), 'retained Pod')
result.update(sourceRemoved=True, retainedAfterImageDeletion=True)
a.results.write_text(json.dumps(result, indent=2) + '\n')
print('retained release remains deployable after source and Image removal', flush=True)

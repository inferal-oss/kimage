#!/usr/bin/env python3
"""Separate real Distribution TLS/Basic-auth target on the owned fixture.

A distinct namespace, PVC, issuer and process keep it independent of the
embedded artifact store. This test prerequisite is never part of default install.
"""
import argparse
import base64
import gzip
import hashlib
import json
import pathlib
import subprocess
import time

p = argparse.ArgumentParser(description=__doc__)
for key in ['kubeconfig', 'bundle', 'directory']:
    p.add_argument('--' + key, type=pathlib.Path, required=True)
p.add_argument('--trusted-inventory', required=True)
a = p.parse_args()
assert 'server: https://127.0.0.1:27643' in a.kubeconfig.read_text()
raw = (a.bundle / 'inventory.json').read_bytes()
assert 'sha256:' + hashlib.sha256(raw).hexdigest() == a.trusted_inventory
inventory = json.loads(raw)
a.directory.mkdir(parents=True, exist_ok=True, mode=0o700)
ns = name = 'kimage-export-target'
origin = 'https://' + name + '.' + ns + '.svc:5443/'
labels = {'fixture.kimage.dev/test': name}
b = ['kubectl', '--kubeconfig', str(a.kubeconfig), '--request-timeout=45s', '-n', ns]
credentials = a.directory / 'credentials'
if not credentials.exists():
    subprocess.run(['python3', 'scripts/prepare-test-credentials.py', '--dir', str(credentials), '--registry-dns', name + '.' + ns + '.svc'], check=True)


def call(args, body=None):
    value = subprocess.run(b + args, input=None if body is None else json.dumps(body).encode(), capture_output=True, timeout=100)
    if value.returncode:
        if body and body.get('kind') == 'Secret':
            raise RuntimeError('fixture Secret operation failed')
        raise RuntimeError(value.stderr.decode()[:1800])
    return value.stdout


def owned(obj):
    obj['metadata'].setdefault('labels', {}).update(labels)
    current = json.loads(call(['get', obj['kind'], obj['metadata']['name'], '--ignore-not-found=true', '-o', 'json']) or b'null')
    if current:
        assert current['metadata']['labels'].get('fixture.kimage.dev/test') == name
        for field in ['data', 'binaryData', 'immutable', 'type']:
            if field in obj:
                assert current.get(field) == obj[field], field + ' fixture collision'
    else:
        call(['create', '-f', '-'], obj)


def checked(entry):
    assert '/' not in entry['file'] and not entry['file'].startswith('.')
    data = (a.bundle / entry['file']).read_bytes()
    assert len(data) == entry['size'] and 'sha256:' + hashlib.sha256(data).hexdigest() == entry['digest']
    return data


owned(dict(apiVersion='v1', kind='Namespace', metadata=dict(name=ns)))
secret_data = {n: base64.b64encode((credentials / n).read_bytes()).decode() for n in ['tls.crt', 'tls.key']}
password = (credentials / 'publisher-password').read_bytes()
htpasswd = subprocess.check_output(['/usr/sbin/htpasswd', '-niB', 'publisher'], input=password + b'\n', stderr=subprocess.DEVNULL)
# bcrypt salts are intentionally nondeterministic; preserve the first fixture
# identity for resumable setup rather than replacing the live password file.
path = a.directory / 'htpasswd'
if path.exists():
    htpasswd = path.read_bytes()
else:
    path.write_bytes(htpasswd)
    path.chmod(0o600)
secret_data['htpasswd'] = base64.b64encode(htpasswd).decode()
owned(dict(apiVersion='v1', kind='Secret', metadata=dict(name=name + '-credentials'), type='Opaque', data=secret_data))
config = '''version: 0.1
log:
  level: warn
storage:
  filesystem:
    rootdirectory: /data
  delete:
    enabled: false
auth:
  htpasswd:
    realm: export-fixture
    path: /credentials/htpasswd
http:
  addr: ':5443'
  tls:
    certificate: /credentials/tls.crt
    key: /credentials/tls.key
'''
owned(dict(apiVersion='v1', kind='ConfigMap', metadata=dict(name=name + '-config'), immutable=True, data={'config.yml': config}))
owned(dict(apiVersion='v1', kind='PersistentVolumeClaim', metadata=dict(name=name + '-data'), spec=dict(accessModes=['ReadWriteOnce'], storageClassName='local-path', resources=dict(requests=dict(storage='1Gi')))))
launcher = checked(inventory['launcher'])
assert len(launcher) <= 512 * 1024
owned(dict(apiVersion='v1', kind='ConfigMap', metadata=dict(name=name + '-launcher'), immutable=True, binaryData=dict(launcher=base64.b64encode(launcher).decode())))
payload = gzip.compress(checked(inventory['components']['registry']['executable']), compresslevel=1, mtime=0)
chunks = [payload[index:index + 512 * 1024] for index in range(0, len(payload), 512 * 1024)]
assert 1 <= len(chunks) <= 256
sources = []
for index, chunk in enumerate(chunks):
    chunk_name = name + '-payload-' + str(index).zfill(4)
    key = str(index).zfill(4)
    owned(dict(apiVersion='v1', kind='ConfigMap', metadata=dict(name=chunk_name), immutable=True, binaryData={key: base64.b64encode(chunk).decode()}))
    sources.append(dict(configMap=dict(name=chunk_name)))
carrier = 'docker.io/rancher/mirrored-pause@sha256:f548e0e8e3dc1896ca956272154dde3314e8cc4fde0a57577ee9fa1c63f5baf4'
mounts = [dict(name=n, mountPath=path, readOnly=ro) for n, path, ro in [('launcher', '/bootstrap', True), ('payload', '/payload', True), ('config', '/config', True), ('credentials', '/credentials', True), ('work', '/work', False), ('data', '/data', False)]]
volumes = [dict(name='launcher', configMap=dict(name=name + '-launcher', defaultMode=365)), dict(name='payload', projected=dict(sources=sources)), dict(name='config', configMap=dict(name=name + '-config')), dict(name='credentials', secret=dict(secretName=name + '-credentials', defaultMode=256)), dict(name='work', emptyDir=dict(sizeLimit='128Mi')), dict(name='data', persistentVolumeClaim=dict(claimName=name + '-data'))]
owned(dict(apiVersion='v1', kind='Pod', metadata=dict(name=name), spec=dict(nodeSelector={'kubernetes.io/hostname': 'kimage-clean'}, automountServiceAccountToken=False, volumes=volumes, containers=[dict(name='registry', image=carrier, imagePullPolicy='Never', command=['/bootstrap/launcher'], args=['/payload', str(len(chunks)), hashlib.sha256(payload).hexdigest(), '/work/registry', 'serve', '/config/config.yml'], volumeMounts=mounts, env=[dict(name='OTEL_TRACES_EXPORTER', value='none')], securityContext=dict(runAsUser=0, runAsGroup=0, allowPrivilegeEscalation=False, readOnlyRootFilesystem=True, capabilities=dict(drop=['ALL'])), readinessProbe=dict(tcpSocket=dict(port=5443), timeoutSeconds=3), resources=dict(requests=dict(cpu='100m', memory='128Mi'), limits=dict(cpu='1', memory='512Mi')))])))
owned(dict(apiVersion='v1', kind='Service', metadata=dict(name=name), spec=dict(selector=labels, ports=[dict(port=5443, targetPort=5443)])))
end = time.monotonic() + 300
while time.monotonic() < end:
    pod = json.loads(call(['get', 'pod', name, '-o', 'json']))
    if any(c['type'] == 'Ready' and c['status'] == 'True' for c in pod.get('status', {}).get('conditions', [])):
        break
    time.sleep(2)
else:
    raise RuntimeError('external registry fixture readiness deadline')
(a.directory / 'metadata.json').write_text(json.dumps(dict(origin=origin, repository=name + '.' + ns + '.svc:5443/customer/api', namespace=ns, pod=name, pvcUid=json.loads(call(['get', 'pvc', name + '-data', '-o', 'json']))['metadata']['uid'], registryExecutableVersion='3.1.2'), indent=2) + '\n')
print('separate TLS/Basic-auth Distribution registry ready with independent PVC', flush=True)

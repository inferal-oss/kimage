#!/usr/bin/env python3
"""Scoped regression harness. Source the authorized Bash environment before use.

No runtime injection or node edits. This fixture proves native carrier bootstrap,
byte publication, read/write policy and normal pulls, not a completed milestone.
"""
import argparse
import base64
import hashlib
import http.client
import json
import os
import pathlib
import socket
import ssl
import subprocess
import time
import uuid

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('action', choices=['bootstrap', 'seed', 'probe', 'pull', 'auth', 'recover', 'uninstall', 'audit', 'schema-dryrun', 'verify-remote', 'handover', 'schema-live'])
p.add_argument('--options', type=pathlib.Path, required=True)
p.add_argument('--bundle', type=pathlib.Path, required=True)
p.add_argument('--linux-dir', type=pathlib.Path, required=True)
p.add_argument('--installer', type=pathlib.Path, default=pathlib.Path('target/debug/kimage-install'))
a = p.parse_args()
o = json.loads(a.options.read_text())
if subprocess.check_output(['kubectl', 'config', 'current-context'], text=True).strip() != 'inferal':
    raise SystemExit('requires explicitly authorized inferal context')
ns, install = o['namespace'], o['installation']
if ns != 'dev-yrashk' or not install.startswith('kimage-foundation-'):
    raise SystemExit('test scope must be dev-yrashk and a unique kimage-foundation installation')
kube = ['kubectl', '--request-timeout=20s', '-n', ns]
if os.environ.get('KIMAGE_API_SERVER'):
    kube += ['--server', os.environ['KIMAGE_API_SERVER'], '--tls-server-name', os.environ['KIMAGE_TLS_SERVER_NAME']]
label = 'kimage.dev/installation=' + install
args = [str(a.installer.absolute())]
cluster = []
for key, flag in [('namespace','namespace'),('installation','installation'),('node','node'),('carrier','carrier'),('storageClass','storage-class'),('port','port'),('credentials','credentials')]:
    cluster += ['--' + flag, str(o[key])]
if o.get('pvc'):
    cluster += ['--pvc', o['pvc']]
trusted = '' if a.action == 'schema-dryrun' else 'sha256:' + hashlib.sha256((a.bundle / 'inventory.json').read_bytes()).hexdigest()
bundle_args = ['--bundle', str(a.bundle), '--trusted-inventory', trusted]
origin = f'https://{install}-registry.{ns}.svc:5443/'
credential_dir = pathlib.Path(o['credentials'])
work = a.options.parent


def output(command):
    return subprocess.check_output(command, text=True)


def native(action):
    subprocess.run(args + [action] + cluster + bundle_args, check=True)


if a.action in ['bootstrap', 'seed', 'recover', 'handover']:
    native(a.action)
elif a.action == 'uninstall':
    subprocess.run(args + ['uninstall'] + cluster, check=True)
elif a.action == 'audit':
    print(output(kube + ['get', 'pods,services,configmaps,secrets,pvc', '-l', label, '-o', 'name']), end='')
elif a.action == 'schema-dryrun':
    schemas = json.loads(output(args + ['schemas']))
    for crd in schemas:
        group = 'schema-' + uuid.uuid4().hex[:8] + '.kimage.test'
        crd['spec']['group'] = group
        crd['metadata']['name'] = crd['spec']['names']['plural'] + '.' + group
        fixture = work / (crd['spec']['names']['plural'] + '-dryrun.json')
        fixture.write_text(json.dumps(crd))
        subprocess.run(kube + ['create', '--dry-run=server', '-f', str(fixture), '-o', 'name'], check=True)
elif a.action == 'schema-live':
    # Unique API group isolates structural/immutability tests from retained CRDs.
    group = 'foundation-' + uuid.uuid4().hex[:8] + '.kimage.test'
    schemas = json.loads(output(args + ['schemas']))
    created = []
    try:
        for crd in schemas:
            crd['spec']['group'] = group
            crd['metadata']['name'] = crd['spec']['names']['plural'] + '.' + group
            crd['metadata']['labels'] = {'kimage.dev/installation': install}
            file = work / (crd['spec']['names']['plural'] + '-isolated.json')
            file.write_text(json.dumps(crd))
            subprocess.run(kube + ['create', '-f', str(file), '-o', 'name'], check=True)
            created.append(crd['metadata']['name'])
            subprocess.run(kube + ['wait', '--for=condition=Established', 'crd/' + created[-1], '--timeout=30s'], check=True)
        root = json.loads((work / 'probe' / 'probe.root.json').read_text())
        spec = dict(version='1.0.0', channels={}, platform=dict(os='linux',architecture='amd64'), source=dict(ociArchive=dict(claimName=install+'-data', path='input/image.tar', root=root, producerHandoff=True)))
        fixture = work / 'schema-image.json'
        obj = dict(apiVersion=group+'/v1alpha1',kind='Image',metadata=dict(name=install+'-schema',namespace=ns),spec=spec)
        fixture.write_text(json.dumps(obj))
        subprocess.run(kube + ['create','--dry-run=server','-f',str(fixture),'-o','name'],check=True)
        negatives = 0
        for change in ['union','version','path']:
            bad = json.loads(json.dumps(obj))
            if change == 'union': bad['spec']['source']['build'] = dict(format='Containerfile')
            if change == 'version': bad['spec']['version'] = '1.0.0-01'
            if change == 'path': bad['spec']['source']['ociArchive']['path'] = 'input/../escape'
            fixture.write_text(json.dumps(bad))
            result = subprocess.run(kube+['create','--dry-run=server','-f',str(fixture),'-o','name'],stdout=subprocess.PIPE,stderr=subprocess.PIPE)
            assert result.returncode != 0, change + ' was accepted'
            negatives += 1
        rs = dict(imageName=install+'-schema',imageUid=str(uuid.uuid4()),version='1.0.0',snapshot='sha256:'+'a'*64,requestDigest='sha256:'+'b'*64,root=root,platform=dict(os='linux',architecture='amd64'),sourceGeneration=1,publisherVersion='0.1.0')
        release = dict(apiVersion=group+'/v1alpha1',kind='ImageRelease',metadata=dict(name=install+'-schema-release',namespace=ns),spec=rs)
        file = work / 'schema-release.json';file.write_text(json.dumps(release))
        subprocess.run(kube+['create','-f',str(file),'-o','name'],check=True)
        changed = dict(root=dict(root, digest='sha256:'+'c'*64))
        result = subprocess.run(kube+['patch','imagereleases.'+group,release['metadata']['name'],'--dry-run=server','--type=merge','-p',json.dumps(dict(spec=changed))],stdout=subprocess.PIPE,stderr=subprocess.PIPE)
        assert result.returncode != 0, 'release mutation accepted'
        (work/'schema-evidence.json').write_text(json.dumps(dict(isolatedApiGroup=group,initialSchemasEstablished=True,validSourceAccepted=True,negativeDeclarationsRejected=negatives,immutableReleaseMutationDenied=True)))
        print('isolated API-group schema validation and immutability verified')
    finally:
        for name in created:
            crd = json.loads(output(kube+['get','crd',name,'-o','json']))
            assert crd['metadata']['labels']['kimage.dev/installation'] == install
            subprocess.run(kube+['delete','crd',name,'--wait=true','--timeout=30s','-o','name'],check=True)
elif a.action == 'probe':
    nonce = 'kimage-foundation-' + uuid.uuid4().hex
    fixture = work / 'probe'
    subprocess.run(['python3', 'scripts/package-bundle.py', '--linux-dir', str(a.linux_dir), '--registry', '.local/upstream/registry', '--out', str(fixture), '--probe-nonce', nonce], check=True)
    repository = f'{ns}/system-probe'
    subprocess.run(args + ['publish', '--archive', str(fixture / 'probe.oci.tar'), '--root', str(fixture / 'probe.root.json'), '--namespace', ns, '--pod', install + '-registry', '--origin', origin, '--repository', repository, '--ca', str(credential_dir / 'ca.crt'), '--password-file', str(credential_dir / 'publisher-password')], check=True)
    (work / 'probe-message.txt').write_text(nonce)
elif a.action == 'verify-remote':
    subprocess.run(args + ['verify-remote', '--root', str(work / 'probe' / 'probe.root.json'), '--namespace', ns, '--pod', install + '-registry', '--origin', origin, '--repository', f'{ns}/system-probe', '--ca', str(credential_dir / 'ca.crt'), '--password-file', str(credential_dir / 'publisher-password')], check=True)
elif a.action == 'pull':
    root = json.loads((work / 'probe' / 'probe.root.json').read_text())
    name = install + '-probe-' + uuid.uuid4().hex[:4]
    pod = dict(apiVersion='v1', kind='Pod', metadata=dict(name=name, labels={'kimage.dev/installation':install}), spec=dict(nodeName=o['node'], automountServiceAccountToken=False, restartPolicy='Never', containers=[dict(name='probe', image=f'127.0.0.1:{o["port"]}/{ns}/system-probe@{root["digest"]}', imagePullPolicy='IfNotPresent', resources=dict(requests=dict(cpu='10m', memory='8Mi'), limits=dict(cpu='100m', memory='32Mi')), securityContext=dict(allowPrivilegeEscalation=False,readOnlyRootFilesystem=True,capabilities=dict(drop=['ALL'])))]))
    fixture = work / 'probe-pod.json'
    fixture.write_text(json.dumps(pod))
    subprocess.run(kube + ['create', '-f', str(fixture), '-o', 'name'], check=True)
    subprocess.run(kube + ['wait', '--for=condition=Ready', 'pod/' + name, '--timeout=120s'], check=True)
    log = output(kube + ['logs', name])
    assert (work / 'probe-message.txt').read_text() in log
    proxy_log = output(kube + ['logs', install + '-proxy'])
    assert root['digest'] in proxy_log and 'registry read' in proxy_log
    evidence = dict(root=root, pod=name, phase='Running', normalPullPolicy='IfNotPresent', registryRootRequestObserved=True)
    (work / 'pull-evidence.json').write_text(json.dumps(evidence, indent=2))
    print('fresh normal kubelet pull and registry request verified')
elif a.action == 'auth':
    # API tunnel still validates the registry certificate's Service identity.
    with socket.socket() as s:
        s.bind(('127.0.0.1', 0))
        port = s.getsockname()[1]
    log = open(work / 'auth-forward.log', 'w')
    forward = subprocess.Popen(kube + ['port-forward', 'pod/' + install + '-registry', f'{port}:5443'], stdout=log, stderr=log)
    context = ssl.create_default_context(cafile=str(credential_dir / 'ca.crt'))
    host = f'{install}-registry.{ns}.svc'
    class Connection(http.client.HTTPSConnection):
        def connect(self):
            self.sock = context.wrap_socket(socket.create_connection(('127.0.0.1', port), timeout=5), server_hostname=host)
    def request(user, method, path):
        secret = (credential_dir / (user + '-password')).read_text().strip()
        connection = Connection(host, port, context=context, timeout=10)
        authorization = base64.b64encode((user + ':' + secret).encode()).decode()
        connection.request(method, path, headers={'Authorization':'Basic ' + authorization, 'Content-Length':'0'})
        response = connection.getresponse()
        status, location = response.status, response.getheader('Location')
        response.read()
        connection.close()
        return status, location
    try:
        for _ in range(50):
            try:
                status, _ = request('reader', 'GET', '/v2/')
                break
            except OSError:
                time.sleep(0.1)
        assert status == 200
        path = f'/v2/{ns}/system-probe/blobs/uploads/'
        assert request('reader', 'POST', path)[0] == 403
        status, location = request('publisher', 'POST', path)
        assert status == 202 and location.startswith(path)
        assert request('publisher', 'DELETE', location)[0] == 204
        (work / 'auth-evidence.json').write_text(json.dumps(dict(readerGet=200, readerWrite=403, publisherStart=202, publisherCancel=204)))
        print('separate authenticated read/write identities verified')
    finally:
        forward.terminate()
        forward.wait(timeout=10)
        log.close()

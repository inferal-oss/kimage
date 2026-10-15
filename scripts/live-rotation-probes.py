#!/usr/bin/env python3
"""Verified TLS/auth rotation and canonical proxy authority on the owned fixture."""
import argparse
import base64
import contextlib
import http.client
import json
import pathlib
import re
import socket
import ssl
import subprocess
import time
import urllib.parse

p = argparse.ArgumentParser(description=__doc__)
for name in ['kubeconfig', 'old-credentials', 'new-credentials', 'baseline', 'results']:
    p.add_argument('--' + name, type=pathlib.Path, required=True)
a = p.parse_args()
assert re.search(r'server: https://127\.0\.0\.1:27643\s', a.kubeconfig.read_text())
assert (a.new_credentials / 'ca.crt').read_bytes() != (a.old_credentials / 'ca.crt').read_bytes()
dns = 'kimage-clean-registry.kimage-system.svc'
old = next(r for r in json.loads(a.baseline.read_text())['releases'])
path = '/v2/kimage-app/' + old['spec']['imageUid'] + '/manifests/' + old['spec']['root']['digest']

@contextlib.contextmanager
def forward(pod, remote, local):
    process = subprocess.Popen(['kubectl', '--kubeconfig', str(a.kubeconfig), '-n', 'kimage-system', 'port-forward', '--address=127.0.0.1', pod, str(local) + ':' + str(remote)], stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    try:
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            assert process.poll() is None, 'API port-forward exited'
            try:
                socket.create_connection(('127.0.0.1', local), timeout=0.2).close()
                break
            except OSError:
                time.sleep(0.1)
        else:
            raise RuntimeError('API port-forward readiness deadline')
        yield
    finally:
        process.terminate()
        process.wait(timeout=10)

def request(method, route, role='reader', credentials=None, ca=None):
    credentials = credentials or a.new_credentials
    ca = ca or a.new_credentials / 'ca.crt'
    connection = http.client.HTTPSConnection(dns, 37644, timeout=15, context=ssl.create_default_context(cafile=str(ca)))
    connection._create_connection = lambda *args, **kwargs: socket.create_connection(('127.0.0.1', 37644), timeout=15)
    password = (credentials / (role + '-password')).read_text().rstrip()
    auth = base64.b64encode((role + ':' + password).encode()).decode()
    try:
        connection.request(method, route, headers={'Authorization': 'Basic ' + auth, 'Connection': 'close', 'Accept': 'application/vnd.oci.image.manifest.v1+json, application/vnd.oci.image.index.v1+json, application/vnd.docker.distribution.manifest.v2+json'})
        response = connection.getresponse()
        value = response.status, dict(response.getheaders()), response.read(2 * 1024 * 1024 + 1)
        assert len(value[2]) <= 2 * 1024 * 1024
        return value
    finally:
        connection.close()

with forward('kimage-clean-registry', 5443, 37644):
    status = request('GET', path)[0]
    assert status == 200, 'retained manifest read status ' + str(status)
    for role in ['reader', 'publisher']:
        assert request('GET', path, role=role, credentials=a.old_credentials)[0] == 401
    try:
        request('GET', path, ca=a.old_credentials / 'ca.crt')
    except ssl.SSLCertVerificationError:
        pass
    else:
        raise AssertionError('obsolete trust root accepted new registry leaf')
    upload = '/v2/kimage-system/system-rotation/blobs/uploads/'
    assert request('POST', upload)[0] == 403
    status, headers, _ = request('POST', upload, role='publisher')
    assert status == 202
    location = next(value for key, value in headers.items() if key.lower() == 'location')
    parsed = urllib.parse.urlsplit(location)
    assert parsed.path.startswith(upload) and parsed.path != upload
    # The opaque signed upload URL stays in process memory, never results/logs.
    route = parsed.path + ('?' + parsed.query if parsed.query else '')
    assert request('DELETE', route, role='publisher')[0] == 204
    assert request('GET', path)[0] == 200
with forward('kimage-clean-proxy', 17641, 37641):
    def proxy(host):
        connection = http.client.HTTPConnection('127.0.0.1', 37641, timeout=10)
        try:
            connection.request('GET', '/v2/', headers={'Host': host, 'Connection': 'close'})
            response = connection.getresponse()
            response.read(4096)
            return response.status
        finally:
            connection.close()
    assert proxy('127.0.0.1:17641') == 200
    for host in ['localhost:17641', '127.0.0.2:17641', 'arbitrary.invalid:17641']:
        assert proxy(host) == 403
a.results.write_text(json.dumps(dict(newCaVerified=True, obsoleteCaRejected=True, obsoleteReaderAndPublisherPasswordsRejected401=True, newReaderRead200=True, readerWriteForbidden403=True, newPublisherUpload202AndOwnedUploadDelete204=True, retainedGraphReadPreserved=True, proxyCanonicalAuthorityRead200=True, proxyAliasAuthoritiesForbidden403=True), indent=2) + '\n')
print('real TLS rotation, separate read/write auth and canonical proxy authority passed', flush=True)

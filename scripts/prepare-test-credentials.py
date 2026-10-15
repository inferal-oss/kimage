#!/usr/bin/env python3
"""Explicit local test issuer, with separate CA and server leaf. No secret stdout."""
import argparse
import os
import pathlib
import re
import secrets
import shutil
import subprocess
p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--dir', type=pathlib.Path, required=True)
p.add_argument('--registry-dns', required=True)
p.add_argument('--admission-dns')
p.add_argument('--ca-cert', type=pathlib.Path)
p.add_argument('--ca-key', type=pathlib.Path)
a = p.parse_args()
names = [a.registry_dns] + ([a.admission_dns] if a.admission_dns else [])
assert all(re.fullmatch(r'[a-z0-9.-]{1,253}', n) for n in names), 'invalid DNS name'
assert bool(a.ca_cert) == bool(a.ca_key), 'CA certificate and key must be paired'
a.dir.mkdir(parents=True, exist_ok=True)
os.chmod(a.dir, 0o700)
for name in ['reader-password', 'publisher-password', 'resolution-key', 'ca.crt', 'ca.key', 'tls.crt', 'tls.key']:
    if (a.dir / name).exists():
        raise SystemExit('refusing to replace existing credentials')
for name in ['reader-password', 'publisher-password', 'resolution-key']:
    path = a.dir / name
    path.write_text(secrets.token_hex(32))
    os.chmod(path, 0o600)
def openssl(args):
    subprocess.run(['openssl'] + args, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
if a.ca_cert:
    shutil.copyfile(a.ca_cert, a.dir / 'ca.crt')
    shutil.copyfile(a.ca_key, a.dir / 'ca.key')
else:
    openssl(['req', '-x509', '-newkey', 'rsa:3072', '-nodes', '-keyout', str(a.dir / 'ca.key'), '-out', str(a.dir / 'ca.crt'), '-days', '7', '-subj', '/CN=kimage-test-root', '-addext', 'basicConstraints=critical,CA:TRUE', '-addext', 'keyUsage=critical,keyCertSign,cRLSign'])
openssl(['req', '-new', '-newkey', 'rsa:3072', '-nodes', '-keyout', str(a.dir / 'tls.key'), '-out', str(a.dir / 'tls.csr'), '-subj', '/CN=' + names[0]])
ext = a.dir / 'leaf.ext'
ext.write_text('basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=' + ','.join('DNS:' + n for n in names) + '\n')
openssl(['x509', '-req', '-in', str(a.dir / 'tls.csr'), '-CA', str(a.dir / 'ca.crt'), '-CAkey', str(a.dir / 'ca.key'), '-CAcreateserial', '-CAserial', str(a.dir / 'ca.srl'), '-out', str(a.dir / 'tls.crt'), '-days', '7', '-extfile', str(ext)])
for name in ['tls.key', 'ca.key']:
    os.chmod(a.dir / name, 0o600)
ext.unlink()
(a.dir / 'tls.csr').unlink()
print('test credentials created; server leaf expires after seven days')

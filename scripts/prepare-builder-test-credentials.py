#!/usr/bin/env python3
"""Private fixture issuer for mutually authenticated BuildKit; no secret stdout."""
import argparse
import os
import pathlib
import re
import subprocess

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--dir', type=pathlib.Path, required=True)
p.add_argument('--dns', required=True)
a = p.parse_args()
assert re.fullmatch(r'[a-z0-9.-]{1,253}', a.dns), 'invalid service DNS'
a.dir.mkdir(parents=True, exist_ok=True)
os.chmod(a.dir, 0o700)
assert not any(a.dir.iterdir()), 'refusing to replace credentials'

def openssl(args):
    subprocess.run(['openssl'] + args, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

openssl(['req', '-x509', '-newkey', 'rsa:3072', '-nodes', '-keyout', str(a.dir / 'ca.key'), '-out', str(a.dir / 'ca.crt'), '-days', '7', '-subj', '/CN=kimage-builder-test-root', '-addext', 'basicConstraints=critical,CA:TRUE', '-addext', 'keyUsage=critical,keyCertSign,cRLSign'])
for role in ['server', 'client']:
    openssl(['req', '-new', '-newkey', 'rsa:3072', '-nodes', '-keyout', str(a.dir / (role + '.key')), '-out', str(a.dir / (role + '.csr')), '-subj', '/CN=' + (a.dns if role == 'server' else 'kimage-direct')])
    extension = a.dir / (role + '.ext')
    extension.write_text('basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=' + ('serverAuth\nsubjectAltName=DNS:' + a.dns + '\n' if role == 'server' else 'clientAuth\n'))
    openssl(['x509', '-req', '-in', str(a.dir / (role + '.csr')), '-CA', str(a.dir / 'ca.crt'), '-CAkey', str(a.dir / 'ca.key'), '-CAcreateserial', '-CAserial', str(a.dir / 'ca.srl'), '-out', str(a.dir / (role + '.crt')), '-days', '7', '-extfile', str(extension)])
    extension.unlink()
    (a.dir / (role + '.csr')).unlink()
for path in a.dir.iterdir():
    os.chmod(path, 0o600)
print('separate builder server/client test identities created; seven-day expiry')

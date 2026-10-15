#!/usr/bin/env python3
"""Activate the optional exporter only for the explicit owned target origin."""
import argparse
import hashlib
import json
import os
import pathlib
import subprocess
p = argparse.ArgumentParser(description=__doc__)
for name in ['kubeconfig', 'options', 'bundle', 'installer', 'target']:
    p.add_argument('--' + name, type=pathlib.Path, required=True)
a = p.parse_args()
assert 'server: https://127.0.0.1:27643' in a.kubeconfig.read_text()
options = json.loads(a.options.read_text())
assert options['installation'] == 'kimage-clean'
metadata = json.loads((a.target / 'metadata.json').read_text())
assert metadata['origin'] == 'https://kimage-export-target.kimage-export-target.svc:5443/'
args = [str(a.installer.resolve()), 'exporter']
for key, value in options.items():
    args += ['--' + ''.join('-' + c.lower() if c.isupper() else c for c in key), str(value)]
args += ['--bundle', str(a.bundle.resolve()), '--trusted-inventory', 'sha256:' + hashlib.sha256((a.bundle / 'inventory.json').read_bytes()).hexdigest(), '--approved-origin', metadata['origin']]
subprocess.run(args, env=dict(os.environ, KUBECONFIG=str(a.kubeconfig.resolve())), check=True, timeout=600)
print('optional exporter activated for the explicit private registry origin', flush=True)

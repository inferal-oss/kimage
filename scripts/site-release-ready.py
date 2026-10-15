#!/usr/bin/env python3
"""Hold new-version website instructions until crates and bundle assets are public."""
import json
import os
from pathlib import Path
import re
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parents[1]


def get(url, github=False):
    headers = {'User-Agent': 'kimage-site (https://github.com/inferal-oss/kimage)'}
    if github and os.environ.get('GH_TOKEN'):
        headers['Authorization'] = 'Bearer ' + os.environ['GH_TOKEN']
    try:
        with urllib.request.urlopen(urllib.request.Request(url, headers=headers), timeout=30) as response:
            return json.load(response)
    except urllib.error.HTTPError as error:
        if error.code == 404:
            return None
        raise


def ready(version):
    for name in ('kimage-core', 'kimage-control'):
        result = get(f'https://crates.io/api/v1/crates/{name}/{version}')
        if result is None or result['version']['yanked']:
            return False
    release = get(f'https://api.github.com/repos/inferal-oss/kimage/releases/tags/v{version}', github=True)
    if release is None or release['draft']:
        return False
    if version == '0.1.0':
        return True
    names = {asset['name'] for asset in release['assets'] if asset['state'] == 'uploaded' and asset.get('digest')}
    return {f'kimage-bundle-v{version}-linux-amd64.tar.gz', f'kimage-bundle-v{version}-linux-amd64.inventory.json'}.issubset(names)


if __name__ == '__main__':
    version = re.search(r'(?ms)^\[workspace\.package\]\s*\n.*?^version\s*=\s*"([^"]+)"', (ROOT / 'Cargo.toml').read_text())[1]
    available = ready(version)
    with open(os.environ['GITHUB_OUTPUT'], 'a') as output:
        output.write('ready=' + str(available).lower() + '\n')
    print('Release available for site deployment.' if available else f'Site validated; deployment awaits public crates and bundles for {version}.')

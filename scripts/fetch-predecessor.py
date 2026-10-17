#!/usr/bin/env python3
"""Fetch the immutable published 0.6.0 predecessor for real upgrade acceptance.

Only metadata requests may carry GitHub authentication. Public asset downloads
are independently checked against GitHub's recorded SHA-256 and size.
"""
import argparse
import hashlib
import json
import os
import re
from pathlib import Path
import urllib.request

VERSION = '0.6.0'
API = 'https://api.github.com/repos/inferal-oss/kimage/releases/tags/v'+VERSION
ASSETS = 'https://github.com/inferal-oss/kimage/releases/download/v'+VERSION+'/'


class NoMetadataRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        raise RuntimeError('GitHub metadata redirect refused')


def fetch(architecture, output, system='linux'):
    assert architecture in ('amd64', 'arm64') and system in ('linux', 'darwin')
    headers = {'Accept':'application/vnd.github+json', 'User-Agent':'kimage-upgrade-acceptance'}
    token = os.environ.get('KIMAGE_GITHUB_TOKEN')
    if token:
        headers['Authorization'] = 'Bearer '+token
    with urllib.request.build_opener(NoMetadataRedirect()).open(urllib.request.Request(API, headers=headers), timeout=60) as response:
        metadata = response.read(1024*1024+1)
    assert len(metadata) <= 1024*1024
    release = json.loads(metadata)
    assert release['tag_name'] == 'v'+VERSION and not release['draft'] and not release['prerelease']
    output.mkdir(parents=True, exist_ok=True)
    names = [f'kimage-v{VERSION}-{system}-{architecture}', 'component-images.json', f'kimage-{VERSION}.tgz']
    records = []
    for name in names:
        assets = [a for a in release['assets'] if a['name'] == name]
        assert len(assets) == 1
        asset = assets[0]
        assert asset['state'] == 'uploaded' and 0 < asset['size'] <= 32*1024*1024
        assert asset['browser_download_url'] == ASSETS+name
        digest = asset.get('digest', '')
        assert re.fullmatch(r'sha256:[0-9a-f]{64}',digest)
        path = output/name
        data = path.read_bytes() if path.is_file() else None
        if data is None or len(data) != asset['size'] or 'sha256:'+hashlib.sha256(data).hexdigest() != digest:
            # Deliberately separate request, with no authentication header.
            with urllib.request.urlopen(urllib.request.Request(ASSETS+name, headers={'User-Agent':'kimage-upgrade-acceptance'}), timeout=120) as response:
                data = response.read(asset['size']+1)
            assert len(data) == asset['size'] and 'sha256:'+hashlib.sha256(data).hexdigest() == digest
            pending = path.with_name(path.name+'.pending')
            with pending.open('wb') as file:
                file.write(data)
            pending.chmod(0o755 if name.startswith('kimage-v') else 0o600)
            pending.replace(path)
        records.append(dict(name=name, size=len(data), sha256=digest))
        print('Verified published predecessor asset: '+name, flush=True)
    (output/'verified.json').write_text(json.dumps(dict(release=VERSION,assets=records), indent=2)+'\n')
    return records


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--architecture', choices=['amd64','arm64'], required=True)
    parser.add_argument('--system', choices=['linux','darwin'], default='linux')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    fetch(args.architecture, args.output, args.system)

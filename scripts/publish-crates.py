#!/usr/bin/env python3
"""Publish verified packages in dependency order, safely resuming partial releases."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import urllib.error
import urllib.request


def published_checksum(name, version):
    request = urllib.request.Request(
        f'https://crates.io/api/v1/crates/{name}/{version}',
        headers={'User-Agent': 'kimage-release (https://github.com/inferal-oss/kimage)'})
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            version = json.load(response)['version']
            assert not version['yanked'], 'refusing to publish a yanked release'
            return version['checksum']
    except urllib.error.HTTPError as error:
        if error.code == 404:
            return None
        raise


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--dry-run', action='store_true')
    args = parser.parse_args()
    subprocess.run(['cargo', 'publish', '--dry-run', '--locked', '--workspace', '--exclude', 'kimage-bootstrap'], check=True)
    if args.dry_run:
        return
    metadata = json.loads(subprocess.check_output(['cargo', 'metadata', '--locked', '--no-deps', '--format-version', '1']))
    packages = {package['name']: package for package in metadata['packages']}
    for name in ('kimage-core', 'kimage-control'):
        version = packages[name]['version']
        archive = Path(metadata['target_directory']) / 'package/tmp-crate' / f'{name}-{version}.crate'
        expected = hashlib.sha256(archive.read_bytes()).hexdigest()
        existing = published_checksum(name, version)
        if existing:
            assert existing == expected, f'{name} {version} already exists with different bytes; refusing replacement'
            print(f'Already published exact archive: {name} {version}', flush=True)
            continue
        subprocess.run(['cargo', 'publish', '--locked', '--package', name], check=True)
        assert published_checksum(name, version) == hashlib.sha256(archive.read_bytes()).hexdigest(), f'{name}: published checksum mismatch'


if __name__ == '__main__':
    main()

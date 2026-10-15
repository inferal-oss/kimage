#!/usr/bin/env python3
"""Stage verified draft assets, then finalize after successful Cargo publication."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess

REPOSITORY = 'inferal-oss/kimage'


def release(tag):
    result = subprocess.run(['gh', 'api', f'repos/{REPOSITORY}/releases/tags/{tag}'], text=True, capture_output=True)
    if result.returncode:
        if 'HTTP 404' in result.stderr:
            return None
        raise RuntimeError(result.stderr)
    return json.loads(result.stdout)


def sha256(path):
    hashed = hashlib.sha256()
    with path.open('rb') as file:
        for block in iter(lambda: file.read(1024 * 1024), b''):
            hashed.update(block)
    return 'sha256:' + hashed.hexdigest()


def verify_assets(value, paths):
    assets = {asset['name']: asset for asset in value['assets']}
    assert len(assets) == len(value['assets']), 'duplicate GitHub asset name'
    for path in paths:
        asset = assets.get(path.name)
        assert asset and asset['state'] == 'uploaded', f'missing release asset: {path.name}'
        assert asset['size'] == path.stat().st_size and asset['digest'] == sha256(path), f'asset checksum conflict: {path.name}'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tag', required=True)
    parser.add_argument('--assets', type=Path, required=True)
    parser.add_argument('--finalize', action='store_true')
    args = parser.parse_args()
    commit = subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip()
    assert subprocess.check_output(['git', 'rev-list', '-n', '1', args.tag], text=True).strip() == commit
    paths = sorted(path for path in args.assets.iterdir() if path.is_file() and path.name != 'RELEASE_NOTES.md')
    assert len(paths) == 7, 'expected bundle, inventory, two installers, license, notices, and checksums'
    value = release(args.tag)
    if value is None:
        assert not args.finalize, 'draft release must be staged first'
        subprocess.run(['gh', 'release', 'create', args.tag, '--repo', REPOSITORY, '--verify-tag', '--target', commit, '--draft', '--title', 'kimage ' + args.tag.removeprefix('v'), '--notes-file', str(args.assets / 'RELEASE_NOTES.md')], check=True)
        value = release(args.tag)
    assert value['tag_name'] == args.tag and value['target_commitish'] == commit, 'release commit mismatch'
    assets = {asset['name']: asset for asset in value['assets']}
    for path in paths:
        if path.name in assets:
            verify_assets(value, [path])
        else:
            assert value['draft'] and not args.finalize, 'cannot add missing assets to a published release'
            subprocess.run(['gh', 'release', 'upload', args.tag, str(path), '--repo', REPOSITORY], check=True)
    value = release(args.tag)
    verify_assets(value, paths)
    if args.finalize and value['draft']:
        subprocess.run(['gh', 'release', 'edit', args.tag, '--repo', REPOSITORY, '--draft=false', '--latest'], check=True)
    print(('Published' if args.finalize else 'Staged') + ' exact release assets for ' + args.tag)


if __name__ == '__main__':
    main()

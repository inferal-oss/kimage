#!/usr/bin/env python3
"""Stage verified draft assets, then finalize after successful Cargo publication."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import time

REPOSITORY = 'inferal-oss/kimage'


def api(path, data=None, method='POST'):
    arguments = ['gh', 'api', '-H', 'Cache-Control: no-cache']
    if data is not None:
        arguments.extend(['--method', method, '--input', '-'])
    arguments.append(f'repos/{REPOSITORY}/{path}')
    result = subprocess.run(arguments, input=json.dumps(data) if data is not None else None, text=True, capture_output=True)
    if result.returncode:
        if data is None and 'HTTP 404' in result.stderr:
            return None
        raise RuntimeError(result.stderr)
    return json.loads(result.stdout)


def release(tag):
    value = api(f'releases/tags/{tag}')
    if value is not None:
        return value
    # The by-tag endpoint omits drafts. Authenticated listing includes them,
    # so interrupted staging can resume without creating a duplicate draft.
    for page in range(1, 11):
        values = api(f'releases?per_page=100&page={page}')
        assert isinstance(values, list), 'could not list release drafts'
        matches = [value for value in values if value['tag_name'] == tag]
        assert len(matches) <= 1, 'duplicate release tag'
        if matches:
            return matches[0]
        if len(values) < 100:
            return None
    raise RuntimeError('release lookup exceeded 1000 entries; refusing to create another draft')


def await_created_release(tag, expected_id):
    # A successful create response can precede visibility in the tag/list index.
    # Never create a second draft while waiting for the first one to appear.
    for delay in (0, 1, 2, 4, 8, 16, 30):
        if delay:
            time.sleep(delay)
        value = release(tag)
        if value is not None:
            assert value['id'] == expected_id, 'created release lookup conflict'
            return value
    raise RuntimeError(f'created draft {expected_id} is not indexed yet; resume it rather than creating another')


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
    architectures = ['amd64', 'arm64'] if tuple(map(int, args.tag.removeprefix('v').split('-')[0].split('.'))) >= (0, 3, 0) else ['amd64']
    expected = {'LICENSE', 'THIRD-PARTY-NOTICES.txt', 'SHA256SUMS'}
    if tuple(map(int,args.tag.removeprefix('v').split('-')[0].split('.'))) >= (0,4,0):
        expected.update(['component-images.json',f'kimage-{args.tag.removeprefix("v")}.tgz'])
    else:
        expected.update(f'kimage-bundle-{args.tag}-linux-{arch}{suffix}' for arch in architectures for suffix in ['.tar.gz', '.inventory.json'])
    platforms = ['linux-amd64', 'linux-arm64', 'darwin-amd64', 'darwin-arm64'] if len(architectures) == 2 else ['linux-amd64', 'darwin-arm64']
    expected.update(f'kimage-{args.tag}-{platform}' for platform in platforms)
    assert {path.name for path in paths} == expected, 'release asset set is incomplete'
    value = release(args.tag)
    if value is None:
        assert not args.finalize, 'draft release must be staged first'
        value = api('releases', {'tag_name': args.tag, 'target_commitish': commit, 'draft': True,
            'name': 'kimage ' + args.tag.removeprefix('v'), 'body': (args.assets / 'RELEASE_NOTES.md').read_text()})
        print('Created draft release ' + str(value['id']), flush=True)
        await_created_release(args.tag, value['id'])
    assert value['tag_name'] == args.tag and value['target_commitish'] == commit, 'release commit mismatch'
    assets = {asset['name']: asset for asset in value['assets']}
    for path in paths:
        if path.name in assets:
            verify_assets(value, [path])
        else:
            assert value['draft'] and not args.finalize, 'cannot add missing assets to a published release'
            subprocess.run(['gh', 'release', 'upload', args.tag, str(path), '--repo', REPOSITORY], check=True)
    value = api(f"releases/{value['id']}")
    verify_assets(value, paths)
    if args.finalize and value['draft']:
        value = api(f"releases/{value['id']}", {'draft': False, 'make_latest': 'true'}, method='PATCH')
        assert not value['draft'] and value['tag_name'] == args.tag
        verify_assets(value, paths)
    print(('Published' if args.finalize else 'Staged') + ' exact release assets for ' + args.tag)


if __name__ == '__main__':
    main()

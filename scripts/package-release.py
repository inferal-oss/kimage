#!/usr/bin/env python3
"""Make deterministic release assets from a natively verified component bundle."""
import argparse
import gzip
import hashlib
import json
from pathlib import Path
import re
import shutil
import tarfile

ROOT = Path(__file__).resolve().parents[1]


def sha256(path):
    value = hashlib.sha256()
    with path.open('rb') as file:
        for block in iter(lambda: file.read(1024 * 1024), b''):
            value.update(block)
    return value.hexdigest()


def files_for(bundle, version):
    inventory = json.loads((bundle / 'inventory.json').read_text())
    assert inventory['schema'] == 1 and inventory['platform'] == {'os': 'linux', 'architecture': 'amd64'}
    entries = [inventory['launcher'], *inventory['installers'].values()]
    for name, component in inventory['components'].items():
        if name in ('operator', 'admission', 'input', 'exporter'):
            assert component['version'] == version, 'component version does not match release'
        entries.extend((component['executable'], component['archive']))
    names = {'inventory.json'}
    for entry in entries:
        name = entry['file']
        assert re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9._-]{0,127}', name) and name not in names
        names.add(name)
        path = bundle / name
        assert path.is_file() and not path.is_symlink()
        assert path.stat().st_size == entry['size'] and 'sha256:' + sha256(path) == entry['digest']
    assert {path.name for path in bundle.iterdir()} == names, 'unexpected files in component bundle'
    assert set(inventory['installers']) == {'linux-amd64', 'darwin-arm64'}
    assert 'builder' in inventory['components'] and 'exporter' in inventory['components']
    return sorted(names), inventory


def package(bundle, output, version):
    assert re.fullmatch(r'\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?', version)
    names, inventory = files_for(bundle, version)
    output.mkdir(parents=True, exist_ok=True)
    base = 'kimage-bundle-v' + version + '-linux-amd64'
    archive = output / (base + '.tar.gz')
    with archive.open('wb') as file, gzip.GzipFile(filename='', fileobj=file, mode='wb', mtime=0) as compressed:
        with tarfile.open(fileobj=compressed, mode='w|', format=tarfile.USTAR_FORMAT) as tar:
            for name in names:
                path = bundle / name
                info = tarfile.TarInfo(name)
                info.size, info.mtime, info.uid, info.gid = path.stat().st_size, 0, 0, 0
                info.mode = 0o555 if path.stat().st_mode & 0o111 else 0o444
                with path.open('rb') as source:
                    tar.addfile(info, source)
    shutil.copyfile(bundle / 'inventory.json', output / (base + '.inventory.json'))
    for platform, installer in inventory['installers'].items():
        target = output / f'kimage-install-v{version}-{platform}'
        shutil.copyfile(bundle / installer['file'], target)
        target.chmod(0o755)
    shutil.copyfile(ROOT / 'LICENSE', output / 'LICENSE')
    notice = ('kimage ' + version + '\nApache License 2.0.\n\n'
              'The component bundle includes Distribution 3.1.2 (distribution/distribution),\n'
              'BuildKit 0.33.0 (moby/buildkit), and its bundled runc (opencontainers/runc).\n'
              'These upstream projects are Apache-2.0 licensed. See their source repositories\n'
              'for copyright notices, dependency licenses, and corresponding source:\n'
              'https://github.com/distribution/distribution/tree/v3.1.2\n'
              'https://github.com/moby/buildkit/tree/v0.33.0\n'
              'https://github.com/opencontainers/runc\n\n'
              'Rust package dependencies and their declared licenses are recorded below.\n')
    metadata_file = output.parent / 'cargo-metadata.json'
    if metadata_file.exists():
        for package in sorted(json.loads(metadata_file.read_text())['packages'], key=lambda package: (package['name'], package['version'])):
            notice += '\n' + package['name'] + ' ' + package['version'] + ': ' + str(package['license']) + '\n'
            location = Path(package['manifest_path']).parent
            for license_file in sorted(location.iterdir()):
                if license_file.is_file() and license_file.name.upper().startswith(('LICENSE', 'LICENCE', 'COPYING', 'NOTICE', 'COPYRIGHT')):
                    notice += '\n' + license_file.name + '\n' + license_file.read_text(errors='replace') + '\n'
    (output / 'THIRD-PARTY-NOTICES.txt').write_text(notice)
    assets = sorted(path for path in output.iterdir() if path.is_file() and path.name not in ('SHA256SUMS', 'RELEASE_NOTES.md'))
    (output / 'SHA256SUMS').write_text(''.join(sha256(path) + '  ' + path.name + '\n' for path in assets))
    changelog = (ROOT / 'CHANGELOG.md').read_text()
    section = re.search(r'(?ms)^## ' + re.escape(version) + r'[^\n]*\n(.*?)(?=^## |\Z)', changelog)
    assert section, 'release needs changelog notes'
    (output / 'RELEASE_NOTES.md').write_text(section[1].strip() + '\n\n[Handbook](https://kimage.dev/docs/) · [Agent essentials](https://kimage.dev/llms.txt)\n')
    print('Packaged release assets for v' + version)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--bundle', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--version', required=True)
    args = parser.parse_args()
    package(args.bundle, args.out, args.version)

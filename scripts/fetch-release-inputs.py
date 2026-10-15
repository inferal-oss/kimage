#!/usr/bin/env python3
"""Fetch the exact upstream Linux executables used by official release bundles."""
import argparse
import hashlib
from pathlib import Path
import shutil
import tarfile
import tempfile
import urllib.request

INPUTS = {"amd64": (
    ('registry', 'https://github.com/distribution/distribution/releases/download/v3.1.2/registry_3.1.2_linux_amd64.tar.gz',
     16032585, '40df2224d410f72ae425c3371873b078bbdbda3b8b612be9571f0e6751f3acc8',
     {'registry': 'registry'}),
    ('buildkit', 'https://github.com/moby/buildkit/releases/download/v0.33.0/buildkit-v0.33.0.linux-amd64.tar.gz',
     93184536, 'b6242896d343100808dcbe37565caf381e0a444a6a83d7255926bb1519248ead',
     {'bin/buildkitd': 'buildkit/buildkitd', 'bin/buildctl': 'buildkit/buildctl', 'bin/buildkit-runc': 'buildkit/buildkit-runc'}),
), "arm64": (
    ('registry', 'https://github.com/distribution/distribution/releases/download/v3.1.2/registry_3.1.2_linux_arm64.tar.gz',
     14303844, '09d26f88d2c0f161bd1b8bfc6c123571cc3d291dcca8af5477b3cacc4ec95e73', {'registry': 'registry'}),
    ('buildkit', 'https://github.com/moby/buildkit/releases/download/v0.33.0/buildkit-v0.33.0.linux-arm64.tar.gz',
     85923030, 'e5acfb5929f967fde3b925ddb39f79fd481a0e96774c641fab3a0e83950d7bfa',
     {'bin/buildkitd': 'buildkit/buildkitd', 'bin/buildctl': 'buildkit/buildctl', 'bin/buildkit-runc': 'buildkit/buildkit-runc'}),
)}


def verified(path, size, digest):
    if not path.is_file() or path.is_symlink() or path.stat().st_size != size:
        return False
    hashed = hashlib.sha256()
    with path.open('rb') as file:
        for block in iter(lambda: file.read(1024 * 1024), b''):
            hashed.update(block)
    return hashed.hexdigest() == digest


def extract(path, files, output):
    with tarfile.open(path, 'r:gz') as archive:
        members = archive.getmembers()
        for name, target in files.items():
            matches = [entry for entry in members if entry.name == name]
            assert len(matches) == 1 and matches[0].isfile(), f'non-regular/missing upstream executable: {name}'
            entry = matches[0]
            assert 0 < entry.size <= 128 * 1024 * 1024, 'upstream executable exceeds inventory bound'
            destination = output / target
            destination.parent.mkdir(parents=True, exist_ok=True)
            with archive.extractfile(entry) as source, destination.open('wb') as file:
                shutil.copyfileobj(source, file, 1024 * 1024)
            assert destination.stat().st_size == entry.size
            destination.chmod(0o755)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--architecture', choices=['amd64', 'arm64'], default='amd64')
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    for name, url, size, digest, files in INPUTS[args.architecture]:
        archive = args.out / (name + '.tar.gz')
        if not verified(archive, size, digest):
            with tempfile.NamedTemporaryFile(dir=args.out, prefix='.download-') as pending:
                hashed, received = hashlib.sha256(), 0
                with urllib.request.urlopen(url, timeout=60) as response:
                    assert response.status == 200 and response.url.startswith('https://')
                    for block in iter(lambda: response.read(1024 * 1024), b''):
                        received += len(block)
                        assert received <= size, 'upstream download exceeds pinned size'
                        hashed.update(block)
                        pending.write(block)
                assert received == size and hashed.hexdigest() == digest, 'upstream archive checksum mismatch'
                pending.flush()
                shutil.copyfile(pending.name, archive)
        extract(archive, files, args.out)
        print(f'Verified pinned {name}: sha256:{digest}')


if __name__ == '__main__':
    main()

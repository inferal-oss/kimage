#!/usr/bin/env python3
"""Offline component packaging; no network, credentials, cluster or runtime store access."""
import argparse
import hashlib
import gzip
import io
import json
import pathlib
import re
import shutil
import struct
import tarfile

MANIFEST = 'application/vnd.oci.image.manifest.v1+json'
CONFIG = 'application/vnd.oci.image.config.v1+json'
LAYER = 'application/vnd.oci.image.layer.v1.tar'


def digest(data):
    return 'sha256:' + hashlib.sha256(data).hexdigest()


def descriptor(data, media):
    return dict(mediaType=media, digest=digest(data), size=len(data))


def encode(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':')).encode()


def append(archive, name, data, mode=0o444):
    header = tarfile.TarInfo(name)
    header.size, header.mode, header.mtime = len(data), mode, 0
    archive.addfile(header, io.BytesIO(data))


def static_elf(path, architecture="amd64"):
    data = path.read_bytes()
    if not (len(data) >= 64 and data[:6] == b'\x7fELF\x02\x01' and int.from_bytes(data[18:20], 'little') == {'amd64': 62, 'arm64': 183}[architecture]):
        raise ValueError(f'{path.name}: requires Linux/{architecture} ELF')
    offset = struct.unpack_from('<Q', data, 32)[0]
    stride, count = struct.unpack_from('<HH', data, 54)
    for i in range(count):
        if struct.unpack_from('<I', data, offset + i * stride)[0] == 3:
            raise ValueError(f'{path.name}: dynamic interpreter is unsupported')
    return data


def image(binary, target, executable, nonce=None, revision=None, architecture="amd64", labels=None, compress=False, additional=None):
    files = dict(additional or {})
    files[executable] = static_elf(binary, architecture)
    return image_files(files, target, executable, nonce, revision, architecture, labels, compress)


def image_files(files, target, executable, nonce=None, revision=None, architecture="amd64", labels=None, compress=False):
    layer = io.BytesIO()
    with tarfile.open(fileobj=layer, mode='w') as tar:
        for name, data in sorted(files.items()):
            append(tar, name, data, 0o444 if name in ('LICENSE', 'THIRD-PARTY-NOTICES.txt') else 0o555)
        if nonce:
            append(tar, 'fixture-nonce', nonce.encode())
    layer = layer.getvalue()
    configuration = dict(
        Entrypoint=['/' + executable], Env=[] if nonce is None else ['KIMAGE_PROBE_MESSAGE=' + nonce])
    if labels:
        configuration['Labels'] = dict(labels)
    if revision:
        configuration.setdefault('Labels', {})['org.opencontainers.image.revision'] = revision
    config = encode(dict(os='linux', architecture=architecture, config=configuration,
        rootfs=dict(type='layers', diff_ids=[digest(layer)])))
    stored_layer = gzip.compress(layer, mtime=0) if compress else layer
    manifest = encode(dict(schemaVersion=2, mediaType=MANIFEST,
                          config=descriptor(config, CONFIG), layers=[descriptor(stored_layer, LAYER + '+gzip' if compress else LAYER)]))
    root = descriptor(manifest, MANIFEST)
    with tarfile.open(target, 'w') as tar:
        append(tar, 'oci-layout', encode(dict(imageLayoutVersion='1.0.0')))
        append(tar, 'index.json', encode(dict(schemaVersion=2, manifests=[root])))
        for blob in [stored_layer, config, manifest]:
            append(tar, 'blobs/sha256/' + digest(blob)[7:], blob)
    return root


def entry(path):
    data = path.read_bytes()
    return dict(file=path.name, size=len(data), digest=digest(data))


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--architecture', choices=['amd64', 'arm64'], default='amd64')
    p.add_argument('--linux-dir', type=pathlib.Path, required=True)
    p.add_argument('--registry', type=pathlib.Path, required=True)
    p.add_argument('--registry-version', choices=['3.1.1', '3.1.2'], default='3.1.2')
    p.add_argument('--serving-version', choices=['0.1.0', '0.2.0', '0.3.0'], default='0.3.0')
    workspace = (pathlib.Path(__file__).resolve().parents[1] / 'Cargo.toml').read_text()
    component_version = re.search(r'(?ms)^\[workspace\.package\]\s*\n.*?^version\s*=\s*"([^"]+)"', workspace)[1]
    p.add_argument('--component-version', default=component_version,
                   help='version of operator/admission/input/exporter; defaults to workspace version')
    p.add_argument('--gateway', type=pathlib.Path, help='override serving executable, leaving control-plane components unchanged')
    p.add_argument('--proxy', type=pathlib.Path, help='override serving executable, leaving control-plane components unchanged')
    p.add_argument('--out', type=pathlib.Path, required=True)
    p.add_argument('--probe-nonce')
    p.add_argument('--serving-revision', help='OCI packaging revision for registry/gateway/proxy; executable versions remain explicit')
    p.add_argument('--macos-installer', type=pathlib.Path)
    p.add_argument('--macos-amd64-installer', type=pathlib.Path)
    p.add_argument('--include-installers', action='store_true')
    p.add_argument('--buildkit-dir', type=pathlib.Path)
    a = p.parse_args()
    assert re.fullmatch(r'\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?', a.component_version)
    if a.serving_revision:
        assert 1 <= len(a.serving_revision) <= 128 and all(c.isalnum() or c in '.-_' for c in a.serving_revision)
    a.out.mkdir(parents=True, exist_ok=True)
    if a.probe_nonce:
        root = image(a.linux_dir / 'kimage-probe', a.out / 'probe.oci.tar', 'kimage-probe', a.probe_nonce, architecture=a.architecture)
        (a.out / 'probe.root.json').write_bytes(encode(root))
        print(root['digest'])
        return
    components = {}
    for name in ['registry', 'gateway', 'proxy', 'operator', 'admission', 'input', 'exporter']:
        source = a.registry if name == 'registry' else a.linux_dir / ('kimage-' + name)
        if name in ['gateway', 'proxy'] and getattr(a, name):
            source = getattr(a, name)
        static_elf(source, a.architecture)
        executable = a.out / ('registry' if name == 'registry' else 'kimage-' + name)
        shutil.copyfile(source, executable)
        executable.chmod(0o755)
        archive = a.out / (name + '.oci.tar')
        root = image(executable, archive, executable.name, revision=a.serving_revision if name in ['registry', 'gateway', 'proxy'] else None, architecture=a.architecture)
        version = a.registry_version if name == 'registry' else a.serving_version if name in ['gateway', 'proxy'] else a.component_version
        components[name] = dict(version=version,
                                executable=entry(executable), archive=entry(archive), root=root)
    if a.buildkit_dir:
        paths = {name: a.buildkit_dir / name for name in ['buildkitd', 'buildctl', 'buildkit-runc']}
        files = {('kimage-builder' if name == 'buildkitd' else name): static_elf(path, a.architecture) for name, path in paths.items()}
        executable = a.out / 'kimage-builder'
        executable.write_bytes(files['kimage-builder'])
        executable.chmod(0o755)
        archive = a.out / 'builder.oci.tar'
        root = image_files(files, archive, 'kimage-builder', architecture=a.architecture)
        components['builder'] = dict(version='0.33.0', executable=entry(executable), archive=entry(archive), root=root)
    launcher = a.out / 'kimage-bootstrap'
    shutil.copyfile(a.linux_dir / launcher.name, launcher)
    launcher.chmod(0o755)
    static_elf(launcher, a.architecture)
    if launcher.stat().st_size > 512 * 1024:
        raise ValueError('launcher exceeds ConfigMap payload bound')
    installers = {}
    if a.include_installers:
        source = a.linux_dir / 'kimage-install'
        static_elf(source, a.architecture)
        target = a.out / ('kimage-install-linux-' + a.architecture)
        shutil.copyfile(source, target)
        target.chmod(0o755)
        installers['linux-' + a.architecture] = entry(target)
        for architecture, source in [('arm64', a.macos_installer), ('amd64', a.macos_amd64_installer)]:
            if source:
                header = source.read_bytes()[:16]
                cpu = {'amd64': 7, 'arm64': 12}[architecture]
                if header[:4] != bytes([207,250,237,254]) or int.from_bytes(header[4:8], 'little') != 0x1000000 + cpu or header[12:] != bytes([2,0,0,0]):
                    raise ValueError(f'macOS installer must be an executable Mach-O for {architecture}')
                target = a.out / ('kimage-install-darwin-' + architecture)
                shutil.copyfile(source, target)
                target.chmod(0o755)
                installers['darwin-' + architecture] = entry(target)
    inventory = encode(dict(schema=1, apiSchemaRevision=5, installers=installers, platform=dict(os='linux', architecture=a.architecture),
                            launcher=entry(launcher), components=components))
    (a.out / 'inventory.json').write_bytes(inventory)
    print(digest(inventory))  # Deliver this digest through an independently trusted channel.


if __name__ == '__main__':
    main()

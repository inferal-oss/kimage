#!/usr/bin/env python3
"""Offline component packaging; no network, credentials, cluster or runtime store access."""
import argparse
import hashlib
import io
import json
import pathlib
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


def static_elf(path):
    data = path.read_bytes()
    if not (len(data) >= 64 and data[:6] == b'\x7fELF\x02\x01' and data[18:20] == b'\x3e\x00'):
        raise ValueError(f'{path.name}: requires Linux/amd64 ELF')
    offset = struct.unpack_from('<Q', data, 32)[0]
    stride, count = struct.unpack_from('<HH', data, 54)
    for i in range(count):
        if struct.unpack_from('<I', data, offset + i * stride)[0] == 3:
            raise ValueError(f'{path.name}: dynamic interpreter is unsupported')
    return data


def image(binary, target, executable, nonce=None, revision=None):
    return image_files({executable: static_elf(binary)}, target, executable, nonce, revision)


def image_files(files, target, executable, nonce=None, revision=None):
    layer = io.BytesIO()
    with tarfile.open(fileobj=layer, mode='w') as tar:
        for name, data in sorted(files.items()):
            append(tar, name, data, 0o555)
        if nonce:
            append(tar, 'fixture-nonce', nonce.encode())
    layer = layer.getvalue()
    configuration = dict(
        Entrypoint=['/' + executable], Env=[] if nonce is None else ['KIMAGE_PROBE_MESSAGE=' + nonce])
    if revision:
        configuration['Labels'] = {'org.opencontainers.image.revision': revision}
    config = encode(dict(os='linux', architecture='amd64', config=configuration,
        rootfs=dict(type='layers', diff_ids=[digest(layer)])))
    manifest = encode(dict(schemaVersion=2, mediaType=MANIFEST,
                          config=descriptor(config, CONFIG), layers=[descriptor(layer, LAYER)]))
    root = descriptor(manifest, MANIFEST)
    with tarfile.open(target, 'w') as tar:
        append(tar, 'oci-layout', encode(dict(imageLayoutVersion='1.0.0')))
        append(tar, 'index.json', encode(dict(schemaVersion=2, manifests=[root])))
        for blob in [layer, config, manifest]:
            append(tar, 'blobs/sha256/' + digest(blob)[7:], blob)
    return root


def entry(path):
    data = path.read_bytes()
    return dict(file=path.name, size=len(data), digest=digest(data))


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--linux-dir', type=pathlib.Path, required=True)
    p.add_argument('--registry', type=pathlib.Path, required=True)
    p.add_argument('--registry-version', choices=['3.1.1', '3.1.2'], default='3.1.2')
    p.add_argument('--serving-version', choices=['0.1.0', '0.2.0'], default='0.2.0')
    p.add_argument('--gateway', type=pathlib.Path, help='override serving executable, leaving control-plane components unchanged')
    p.add_argument('--proxy', type=pathlib.Path, help='override serving executable, leaving control-plane components unchanged')
    p.add_argument('--out', type=pathlib.Path, required=True)
    p.add_argument('--probe-nonce')
    p.add_argument('--serving-revision', help='OCI packaging revision for registry/gateway/proxy; executable versions remain explicit')
    p.add_argument('--macos-installer', type=pathlib.Path)
    p.add_argument('--include-installers', action='store_true')
    p.add_argument('--buildkit-dir', type=pathlib.Path)
    a = p.parse_args()
    if a.serving_revision:
        assert 1 <= len(a.serving_revision) <= 128 and all(c.isalnum() or c in '.-_' for c in a.serving_revision)
    a.out.mkdir(parents=True, exist_ok=True)
    if a.probe_nonce:
        root = image(a.linux_dir / 'kimage-probe', a.out / 'probe.oci.tar', 'kimage-probe', a.probe_nonce)
        (a.out / 'probe.root.json').write_bytes(encode(root))
        print(root['digest'])
        return
    components = {}
    for name in ['registry', 'gateway', 'proxy', 'operator', 'admission', 'input', 'exporter']:
        source = a.registry if name == 'registry' else a.linux_dir / ('kimage-' + name)
        if name in ['gateway', 'proxy'] and getattr(a, name):
            source = getattr(a, name)
        static_elf(source)
        executable = a.out / ('registry' if name == 'registry' else 'kimage-' + name)
        shutil.copyfile(source, executable)
        executable.chmod(0o755)
        archive = a.out / (name + '.oci.tar')
        root = image(executable, archive, executable.name, revision=a.serving_revision if name in ['registry', 'gateway', 'proxy'] else None)
        version = a.registry_version if name == 'registry' else a.serving_version if name in ['gateway', 'proxy'] else '0.1.0'
        components[name] = dict(version=version,
                                executable=entry(executable), archive=entry(archive), root=root)
    if a.buildkit_dir:
        paths = {name: a.buildkit_dir / name for name in ['buildkitd', 'buildctl', 'buildkit-runc']}
        files = {('kimage-builder' if name == 'buildkitd' else name): static_elf(path) for name, path in paths.items()}
        executable = a.out / 'kimage-builder'
        executable.write_bytes(files['kimage-builder'])
        executable.chmod(0o755)
        archive = a.out / 'builder.oci.tar'
        root = image_files(files, archive, 'kimage-builder')
        components['builder'] = dict(version='0.33.0', executable=entry(executable), archive=entry(archive), root=root)
    launcher = a.out / 'kimage-bootstrap'
    shutil.copyfile(a.linux_dir / launcher.name, launcher)
    launcher.chmod(0o755)
    static_elf(launcher)
    if launcher.stat().st_size > 512 * 1024:
        raise ValueError('launcher exceeds ConfigMap payload bound')
    installers = {}
    if a.include_installers:
        source = a.linux_dir / 'kimage-install'
        static_elf(source)
        target = a.out / 'kimage-install-linux-amd64'
        shutil.copyfile(source, target)
        target.chmod(0o755)
        installers['linux-amd64'] = entry(target)
        if a.macos_installer:
            header = a.macos_installer.read_bytes()[:16]
            if header[:8] != bytes([207,250,237,254,12,0,0,1]) or header[12:] != bytes([2,0,0,0]):
                raise ValueError('macOS installer must be an executable Mach-O for arm64')
            target = a.out / 'kimage-install-darwin-arm64'
            shutil.copyfile(a.macos_installer, target)
            target.chmod(0o755)
            installers['darwin-arm64'] = entry(target)
    inventory = encode(dict(schema=1, apiSchemaRevision=4, installers=installers, platform=dict(os='linux', architecture='amd64'),
                            launcher=entry(launcher), components=components))
    (a.out / 'inventory.json').write_bytes(inventory)
    print(digest(inventory))  # Deliver this digest through an independently trusted channel.


if __name__ == '__main__':
    main()

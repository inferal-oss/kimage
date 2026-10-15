#!/usr/bin/env python3
"""Own a full Linux amd64 VM fixture; restricted networking, no SSH or runtime mounts.

Prerequisite preparation is separate from kimage installation. No kimage images
are included in the guest's runtime stores. Retains its private disk on stop.
"""
import argparse
import hashlib
import json
import os
import pathlib
import shutil
import signal
import subprocess
import time

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('action', choices=['init', 'start', 'stop', 'request'])
p.add_argument('--directory', type=pathlib.Path, required=True)
p.add_argument('--base', type=pathlib.Path)
p.add_argument('--base-sha256')
p.add_argument('--prerequisites', type=pathlib.Path)
p.add_argument('--port', type=int, default=27643)
p.add_argument('--cpus', type=int)
p.add_argument('--memory-mib', type=int)
p.add_argument('--cluster-role', choices=['server', 'agent'])
p.add_argument('--peer-port', type=int, default=27660)
p.add_argument('--operation', choices=['status', 'health', 'images', 'external_probe', 'evict_system_images', 'evict_retained_images'])
p.add_argument('--images', type=pathlib.Path, help='JSON array of fixture system references to evict')
a = p.parse_args()
d = a.directory.resolve()
assets = pathlib.Path(__file__).resolve().parent / 'fixtures/qemu'

def run(args):
    subprocess.run([str(v) for v in args], check=True)

def digest(path):
    h = hashlib.sha256()
    with path.open('rb') as f:
        while block := f.read(1024 * 1024):
            h.update(block)
    return h.hexdigest()

if a.action == 'init':
    assert a.base and a.base_sha256 and a.prerequisites
    assert digest(a.base) == a.base_sha256, 'untrusted fixture base'
    assert not d.exists(), 'choose a new fixture directory; disks are never overwritten'
    d.mkdir(mode=0o700, parents=True)
    (d / 'seed').mkdir()
    (d / 'share').mkdir(mode=0o700)
    (d / 'prerequisites').mkdir()
    # Copy only cluster prerequisites, excluding kimage artifacts.
    for name in ['k3s', 'k3s-airgap-images-amd64.tar.zst', 'sha256sum-amd64.txt']:
        shutil.copyfile(a.prerequisites / name, d / 'prerequisites' / name)
    shutil.copyfile(assets / 'agent.py', d / 'prerequisites/fixture-agent.py')
    shutil.copyfile(assets / 'cluster-network.py', d / 'prerequisites/cluster-network.py')
    user_data = (assets / 'user-data.yaml').read_text()
    if a.cluster_role:
        assert 1024 <= a.peer_port <= 65535 and a.peer_port != a.port
        (d / 'prerequisites/cluster.json').write_text(json.dumps(dict(schema=1, role=a.cluster_role)))
        if a.cluster_role == 'agent':
            token = a.prerequisites / 'join-token'
            assert token.is_file() and 32 <= token.stat().st_size <= 4096
            shutil.copyfile(token, d / 'prerequisites/join-token')
            (d / 'prerequisites/join-token').chmod(0o600)
            user_data = user_data.replace("'10.0.2.2'", "'10.0.3.2'")
            user_data = user_data.replace('via, 10.0.2.2,', 'via, 10.0.3.2,')
        marker = '  - [systemctl, daemon-reload]'
        user_data = user_data.replace(marker, '  - [python3, /mnt/prereqs/cluster-network.py]\n' + marker)
    (d / 'seed/user-data').write_text(user_data)
    (d / 'seed/meta-data').write_text('instance-id: kimage-fixture-' + hashlib.sha256(str(d).encode()).hexdigest()[:16] + '\nlocal-hostname: ' + ('kimage-clean-2' if a.cluster_role == 'agent' else 'kimage-clean') + '\n')
    if shutil.which('hdiutil'):
        run(['hdiutil', 'makehybrid', '-iso', '-joliet', '-default-volume-name', 'cidata', '-o', d / 'seed.iso', d / 'seed'])
    else:
        run(['genisoimage', '-output', d / 'seed.iso', '-volid', 'cidata', '-joliet', '-rock', d / 'seed'])
    run(['qemu-img', 'create', '-f', 'qcow2', '-F', 'qcow2', '-b', a.base.resolve(), d / 'node.qcow2', '20G'])
    (d / 'fixture.json').write_text(json.dumps(dict(port=a.port, base=str(a.base.resolve()), baseSha256=a.base_sha256, clusterRole=a.cluster_role, peerPort=a.peer_port)))
elif a.action == 'start':
    assert not (d / 'pid').exists(), 'fixture already started; verify process before removing stale PID'
    config = json.loads((d / 'fixture.json').read_text())
    port = config['port']
    cpus = a.cpus or config.get('cpus', 2)
    memory = a.memory_mib or config.get('memoryMiB', 4096)
    assert 2 <= cpus <= 8 and 4096 <= memory <= 16384, 'bounded fixture resources required'
    role = a.cluster_role or config.get('clusterRole')
    peer_port = config.get('peerPort', a.peer_port)
    assert role in [None, 'server', 'agent']
    assert 1024 <= peer_port <= 65535 and peer_port != port
    subnet = ',net=10.0.3.0/24' if role == 'agent' else ''
    uplink = f'user,id=fixture,restrict=on{subnet},hostfwd=tcp:127.0.0.1:{port}-:6443'
    command = ['qemu-system-x86_64', '-name', 'kimage-clean-fixture', '-machine', 'q35', '-accel', 'tcg,thread=multi', '-cpu', 'max', '-smp', str(cpus), '-m', str(memory), '-display', 'none', '-serial', 'file:' + str(d / 'console.log'), '-drive', 'file=' + str(d / 'node.qcow2') + ',if=virtio,format=qcow2', '-drive', 'file=' + str(d / 'seed.iso') + ',media=cdrom,readonly=on', '-netdev', uplink, '-device', 'virtio-net-pci,netdev=fixture', '-fsdev', 'local,id=prereqs,path=' + str(d / 'prerequisites') + ',security_model=none,readonly=on', '-device', 'virtio-9p-pci,fsdev=prereqs,mount_tag=prereqs', '-fsdev', 'local,id=output,path=' + str(d / 'share') + ',security_model=none', '-device', 'virtio-9p-pci,fsdev=output,mount_tag=output', '-daemonize', '-pidfile', d / 'pid']
    if role:
        assert json.loads((d / 'prerequisites/cluster.json').read_text()) == dict(schema=1, role=role)
        endpoint = 'listen' if role == 'server' else 'connect'
        mac = '52:54:00:4b:49:01' if role == 'server' else '52:54:00:4b:49:02'
        command += ['-netdev', f'socket,id=cluster,{endpoint}=127.0.0.1:{peer_port}', '-device', f'virtio-net-pci,netdev=cluster,mac={mac},addr=0x8']
    run(command)
    config.update(cpus=cpus, memoryMiB=memory, clusterRole=role, peerPort=peer_port)
    (d / 'fixture.json').write_text(json.dumps(config))
elif a.action == 'stop':
    pid = int((d / 'pid').read_text())
    command = subprocess.check_output(['ps', '-p', str(pid), '-o', 'args='], text=True)
    assert 'qemu-system-x86_64' in command and str(d / 'node.qcow2') in command
    os.kill(pid, signal.SIGTERM)
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        state = subprocess.run(['ps', '-p', str(pid), '-o', 'args='], capture_output=True, text=True)
        if str(d / 'node.qcow2') not in state.stdout:
            break
        time.sleep(0.1)
    else:
        raise RuntimeError('owned fixture process did not stop; disk retained')
    (d / 'pid').unlink(missing_ok=True)
else:
    assert a.operation
    identity = os.urandom(8).hex()
    request = dict(id=identity, operation=a.operation)
    if a.operation in ['evict_system_images', 'evict_retained_images']:
        assert a.images
        request['images'] = json.loads(a.images.read_text())
    (d / 'share/request.json').write_text(json.dumps(request))
    deadline = time.monotonic() + 120
    while time.monotonic() < deadline:
        response = d / 'share/response.json'
        if response.exists():
            value = json.loads(response.read_text())
            if value.get('id') == identity:
                assert 'error' not in value, value.get('error')
                print(json.dumps(value['output'], indent=2))
                break
        time.sleep(0.5)
    else:
        raise RuntimeError('fixture diagnostic action timed out')

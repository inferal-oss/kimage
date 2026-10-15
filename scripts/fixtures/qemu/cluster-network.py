#!/usr/bin/python3
"""Fixed provider setup for two owned VMs; never part of kimage installation.

The private Ethernet peer is another QEMU guest. Both SLIRP uplinks retain
restrict=on. No host bridge, SSH service, runtime socket or product image is added.
"""
import argparse
import json
import os
import pathlib
import subprocess

PREREQUISITES = pathlib.Path('/mnt/prereqs')
OUTPUT = pathlib.Path('/mnt/output')


def run(args):
    subprocess.run(args, check=True, timeout=30, stdout=subprocess.DEVNULL)


def configuration():
    path = PREREQUISITES / 'cluster.json'
    assert path.stat().st_size <= 4096
    value = json.loads(path.read_text())
    assert value == dict(schema=1, role=value['role']) and value['role'] in ['server', 'agent']
    return value['role']


def network(role):
    mac = '52:54:00:4b:49:01' if role == 'server' else '52:54:00:4b:49:02'
    links = json.loads(subprocess.check_output(['ip', '-j', 'link', 'show'], timeout=10))
    match = [link for link in links if link.get('address') == mac]
    assert len(match) == 1, 'owned private NIC missing'
    name = match[0]['ifname']
    if name != 'kcluster':
        run(['ip', 'link', 'set', name, 'down'])
        run(['ip', 'link', 'set', name, 'name', 'kcluster'])
    run(['ip', 'link', 'set', 'kcluster', 'up'])
    address = '192.168.77.10/24' if role == 'server' else '192.168.77.11/24'
    run(['ip', 'address', 'replace', address, 'dev', 'kcluster'])
    gateway = '10.0.2.2' if role == 'server' else '10.0.3.2'
    run(['ip', 'route', 'replace', 'default', 'via', gateway, 'dev', 'enp0s2'])
    if role == 'agent':
        # Preserve the original server's InternalIP and API Service endpoint.
        # Only this exact owned peer address is routed over the private link.
        run(['ip', 'route', 'replace', '10.0.2.15/32', 'via', '192.168.77.10', 'dev', 'kcluster'])


def configure(restart=False):
    role = configuration()
    network(role)
    if role == 'server':
        command = ('server --node-name kimage-clean --node-ip 10.0.2.15 '
            '--node-external-ip 192.168.77.10 --advertise-address 10.0.2.15 '
            '--flannel-iface kcluster --flannel-external-ip '
            '--cluster-cidr 10.42.0.0/16 --service-cidr 10.43.0.0/16 '
            '--tls-san 127.0.0.1 --tls-san 192.168.77.10 '
            '--disable traefik --disable metrics-server --disable servicelb '
            '--write-kubeconfig /mnt/output/config --write-kubeconfig-mode 600')
    else:
        token = PREREQUISITES / 'join-token'
        assert token.is_file() and 32 <= token.stat().st_size <= 4096
        command = ('agent --node-name kimage-clean-2 --node-ip 192.168.77.11 '
            '--node-external-ip 192.168.77.11 --flannel-iface kcluster '
            '--server https://192.168.77.10:6443 --token-file /mnt/prereqs/join-token')
    expected = '''[Unit]
Description=Task-owned isolated Kubernetes fixture
After=network-online.target
Wants=network-online.target
RequiresMountsFor=/mnt/prereqs /mnt/output
[Service]
Type=simple
ExecStartPre=/usr/bin/python3 /mnt/prereqs/cluster-network.py --network-only
ExecStart=/usr/local/bin/k3s ''' + command + '''
StandardOutput=append:/mnt/output/k3s.log
StandardError=append:/mnt/output/k3s.log
Restart=on-failure
[Install]
WantedBy=multi-user.target
'''
    unit = pathlib.Path('/etc/systemd/system/kimage-fixture.service')
    changed = not unit.exists() or unit.read_text() != expected
    if changed:
        if unit.exists():
            assert 'Description=Task-owned isolated Kubernetes fixture' in unit.read_text()
        unit.write_text(expected)
        run(['systemctl', 'daemon-reload'])
        if restart:
            run(['systemctl', 'restart', 'kimage-fixture'])
    if role == 'server':
        token = pathlib.Path('/var/lib/rancher/k3s/server/node-token')
        if token.exists():
            data = token.read_bytes()
            assert 32 <= len(data) <= 4096
            destination = OUTPUT / 'join-token'
            destination.write_bytes(data)
            os.chmod(destination, 0o600)
    return dict(role=role, privateNetworkConfigured=True, serviceChanged=changed)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--network-only', action='store_true')
    args = parser.parse_args()
    if args.network_only:
        network(configuration())
    else:
        print(json.dumps(configure()))

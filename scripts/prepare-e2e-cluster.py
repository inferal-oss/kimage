#!/usr/bin/env python3
"""Create a pinned disposable K3s fixture on a GitHub-hosted Linux runner only."""
import argparse
import hashlib
import os
from pathlib import Path
import platform
import subprocess
import time
import urllib.request

VERSION = 'v1.36.5+k3s1'
PINS = {
    'amd64': [('k3s', 79077538, 'd73847bcd3c5fccef0115b372e2f9a91f3032dc84bbf71518a4617565294d313'),
              ('k3s-airgap-images-amd64.tar.zst', 194444189, '7d939471907df2d4e537a9c308bf94173e453fff4a2e9e77e9ae92a64be3007f')],
    'arm64': [('k3s-arm64', 71434402, '135e34cb9e8a1cfae3cb55577789e93501efe7edc080a8b2630458a85b07721e'),
              ('k3s-airgap-images-arm64.tar.zst', 176489628, 'aeb7b47b0f0cb1356a5f0bc1c11b7c499deceeeafcd411963064e02bfc77da84')],
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--directory', type=Path, required=True)
    args = parser.parse_args()
    assert os.environ.get('GITHUB_ACTIONS') == 'true' and os.environ.get('RUNNER_ENVIRONMENT') == 'github-hosted'
    assert platform.system() == 'Linux', 'ephemeral Linux runner required'
    arch = {'x86_64': 'amd64', 'aarch64': 'arm64'}[platform.machine()]
    directory = args.directory.resolve()
    directory.mkdir(parents=True, exist_ok=False)
    for name, size, checksum in PINS[arch]:
        path = directory / name
        hashed, received = hashlib.sha256(), 0
        with urllib.request.urlopen(f'https://github.com/k3s-io/k3s/releases/download/{VERSION}/{name}', timeout=60) as response, path.open('wb') as file:
            for block in iter(lambda: response.read(1024*1024), b''):
                received += len(block)
                assert received <= size
                hashed.update(block)
                file.write(block)
        assert received == size and hashed.hexdigest() == checksum, 'K3s fixture checksum mismatch'
    executable = directory / PINS[arch][0][0]
    subprocess.run(['sudo', 'install', '-m', '755', str(executable), '/usr/local/bin/k3s'], check=True)
    subprocess.run(['sudo', 'mkdir', '-p', '/var/lib/rancher/k3s/agent/images'], check=True)
    subprocess.run(['sudo', 'cp', str(directory / PINS[arch][1][0]), '/var/lib/rancher/k3s/agent/images/'], check=True)
    kubeconfig = directory / 'config'
    with (directory / 'k3s.log').open('wb') as log:
        process = subprocess.Popen(['sudo', '/usr/local/bin/k3s', 'server', '--node-name', 'kimage-e2e',
            '--tls-san', '127.0.0.1', '--disable', 'traefik', '--disable', 'metrics-server', '--disable', 'servicelb',
            *(['--private-registry', os.environ['KIMAGE_E2E_REGISTRY_CONFIG']] if os.environ.get('KIMAGE_E2E_REGISTRY_CONFIG') else []),
            '--write-kubeconfig', str(kubeconfig), '--write-kubeconfig-mode', '600'], stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
    (directory / 'pid').write_text(str(process.pid))
    deadline = time.monotonic() + 180
    while time.monotonic() < deadline:
        if kubeconfig.exists():
            subprocess.run(['sudo', 'chown', str(os.getuid())+':'+str(os.getgid()), str(kubeconfig)], check=True)
            result = subprocess.run(['k3s', 'kubectl', '--kubeconfig', str(kubeconfig), 'wait', 'node/kimage-e2e', '--for=condition=Ready', '--timeout=5s'], capture_output=True)
            if result.returncode == 0:
                break
        assert process.poll() is None, 'K3s fixture exited'
        time.sleep(2)
    else:
        raise RuntimeError('K3s fixture did not become ready')
    # Only Kubernetes prerequisites are imported; all kimage components must
    # subsequently arrive through its normal registry pulls.
    with open(os.environ['GITHUB_ENV'], 'a') as output:
        output.write('KUBECONFIG=' + str(kubeconfig) + '\n')
        output.write('KIMAGE_E2E_ARCHITECTURE=' + arch + '\n')
    print('Pinned native ' + arch + ' Kubernetes fixture ready')


if __name__ == '__main__':
    main()

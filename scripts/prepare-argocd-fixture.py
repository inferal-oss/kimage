#!/usr/bin/env python3
"""Install a checksum-pinned Argo CD only on a disposable GitHub-hosted fixture."""
import hashlib
import os
from pathlib import Path
import subprocess
import urllib.request

VERSION='v3.5.3'
SIZE=1917766
SHA256='7efe2d6bbc03f63623640f1e4198f16c84009d510fb810ef71e56df1b7614ba9'

def main():
    assert os.environ.get('GITHUB_ACTIONS')=='true' and os.environ.get('RUNNER_ENVIRONMENT')=='github-hosted','disposable GitHub-hosted fixture only'
    assert subprocess.check_output(['k3s','kubectl','get','nodes','-o','jsonpath={.items[*].metadata.name}'],text=True)=='kimage-e2e','unexpected fixture node identity'
    with urllib.request.urlopen(f'https://raw.githubusercontent.com/argoproj/argo-cd/{VERSION}/manifests/install.yaml',timeout=60) as response:data=response.read(SIZE+1)
    assert len(data)==SIZE and hashlib.sha256(data).hexdigest()==SHA256,'Argo CD fixture integrity mismatch'
    path=Path('.local/e2e/argocd-install.yaml');path.write_bytes(data)
    subprocess.run(['k3s','kubectl','create','namespace','argocd'],check=True)
    subprocess.run(['k3s','kubectl','apply','--server-side','-n','argocd','-f',str(path)],check=True)
    for kind,name in [('deployment','argocd-repo-server'),('statefulset','argocd-application-controller')]:
        subprocess.run(['k3s','kubectl','rollout','status',kind+'/'+name,'-n','argocd','--timeout=300s'],check=True)

if __name__=='__main__':main()

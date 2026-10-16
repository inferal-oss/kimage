#!/usr/bin/env python3
"""Start a TLS component registry solely on a disposable GitHub-hosted runner."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import time
import urllib.request
import ssl
import shutil


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--images',type=Path,required=True)
    p.add_argument('--registry',type=Path,required=True)
    p.add_argument('--directory',type=Path,required=True)
    a=p.parse_args()
    assert os.environ.get('GITHUB_ACTIONS')=='true' and os.environ.get('RUNNER_ENVIRONMENT')=='github-hosted'
    directory=a.directory.resolve();directory.mkdir(parents=True,exist_ok=False);os.chmod(directory,0o700)
    def openssl(*args):
        subprocess.run(['openssl',*args],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
    openssl('req','-x509','-newkey','rsa:3072','-nodes','-keyout',str(directory/'ca.key'),'-out',str(directory/'ca.crt'),'-days','2','-subj','/CN=kimage ephemeral registry CA','-addext','basicConstraints=critical,CA:TRUE','-addext','keyUsage=critical,keyCertSign,cRLSign')
    openssl('req','-new','-newkey','rsa:3072','-nodes','-keyout',str(directory/'tls.key'),'-out',str(directory/'tls.csr'),'-subj','/CN=127.0.0.1')
    (directory/'leaf.ext').write_text('basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=IP:127.0.0.1\n')
    openssl('x509','-req','-in',str(directory/'tls.csr'),'-CA',str(directory/'ca.crt'),'-CAkey',str(directory/'ca.key'),'-CAcreateserial','-out',str(directory/'tls.crt'),'-days','2','-extfile',str(directory/'leaf.ext'))
    for name in ['ca.key','tls.key']:os.chmod(directory/name,0o600)
    config=dict(version=0.1,log=dict(level='warn'),storage=dict(filesystem=dict(rootdirectory=str(directory/'data'))),http=dict(addr='127.0.0.1:14901',tls=dict(certificate=str(directory/'tls.crt'),key=str(directory/'tls.key'))))
    (directory/'registry.json').write_text(json.dumps(config))
    with (directory/'registry.log').open('wb') as log:
        proc=subprocess.Popen([str(a.registry.resolve()),'serve',str(directory/'registry.json')],stdout=log,stderr=subprocess.STDOUT,start_new_session=True)
    (directory/'pid').write_text(str(proc.pid))
    context=ssl.create_default_context(cafile=str(directory/'ca.crt'))
    for _ in range(60):
        assert proc.poll() is None,'fixture registry exited'
        try:
            with urllib.request.urlopen('https://127.0.0.1:14901/v2/',context=context,timeout=2) as response:assert response.status==200
            break
        except (OSError,urllib.error.URLError):time.sleep(1)
    else:raise RuntimeError('fixture registry not ready')
    trust=directory/'trust';trust.mkdir();shutil.copyfile(directory/'ca.crt',trust/'ca.crt')
    catalog=json.loads((a.images/'component-images.json').read_bytes())
    for role,entry in catalog['components'].items():
        subprocess.run(['skopeo','copy','--all','--preserve-digests','--dest-cert-dir',str(trust),'oci:'+str((a.images/role).resolve())+':v'+catalog['release'],'docker://127.0.0.1:14901/kimage/'+role+':v'+catalog['release']],check=True)
    catalog['repository']='127.0.0.1:14901/kimage'
    (directory/'component-images.json').write_text(json.dumps(catalog))
    # This trust file belongs only to the ephemeral runner's new K3s fixture.
    (directory/'registries.json').write_text(json.dumps(dict(configs={'127.0.0.1:14901':dict(tls=dict(ca_file=str(directory/'ca.crt')))})))
    with open(os.environ['GITHUB_ENV'],'a') as output:
        output.write('KIMAGE_E2E_REGISTRY_CONFIG='+str(directory/'registries.json')+'\n')
        output.write('KIMAGE_E2E_IMAGE_CATALOG='+str(directory/'component-images.json')+'\n')
    print('Ephemeral TLS registry serves complete native indexes; no runtime image imports.')


if __name__=='__main__':main()

#!/usr/bin/env python3
"""Publish immutable OCI indexes using the standard registry protocol, then verify public access."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request

REPOSITORY='ghcr.io/inferal-oss/kimage'
ACCEPT='application/vnd.oci.image.index.v1+json, application/vnd.oci.image.manifest.v1+json'


def public_json(role,kind,digest):
    scope='repository:inferal-oss/kimage/'+role+':pull'
    url='https://ghcr.io/token?'+urllib.parse.urlencode(dict(service='ghcr.io',scope=scope))
    with urllib.request.urlopen(url,timeout=30) as response:token=json.load(response)['token']
    request=urllib.request.Request(f'https://ghcr.io/v2/inferal-oss/kimage/{role}/{kind}/{digest}',headers={'Authorization':'Bearer '+token,'Accept':ACCEPT})
    with urllib.request.urlopen(request,timeout=30) as response:data=response.read(1024*1024+1)
    assert len(data)<=1024*1024 and 'sha256:'+hashlib.sha256(data).hexdigest()==digest,'public component digest mismatch'
    return json.loads(data)


def verify(catalog):
    assert catalog['repository']==REPOSITORY
    for role,entry in catalog['components'].items():
        root=public_json(role,'manifests',entry['digest'])
        assert root['mediaType']=='application/vnd.oci.image.index.v1+json'
        assert [child['platform'] for child in root['manifests']]==entry['platforms']
        for child in root['manifests']:
            manifest=public_json(role,'manifests',child['digest'])
            config=public_json(role,'blobs',manifest['config']['digest'])
            assert all(config[key]==child['platform'][key] for key in ['os','architecture'])
            assert config['config']['Labels']['org.opencontainers.image.version']==entry['version']
        print('Public multi-platform image verified: '+role,flush=True)


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--images',type=Path,required=True);p.add_argument('--verify-only',action='store_true');p.add_argument('--public-wait',type=int,default=0);a=p.parse_args()
    catalog=json.loads((a.images/'component-images.json').read_bytes());assert catalog['repository']==REPOSITORY
    if not a.verify_only:
        with tempfile.TemporaryDirectory(prefix='kimage-registry-auth-') as temp:
            auth=Path(temp)/'auth.json'
            subprocess.run(['skopeo','login','--authfile',str(auth),'--username',os.environ['GITHUB_ACTOR'],'--password-stdin','ghcr.io'],input=os.environ['GHCR_TOKEN'],text=True,check=True,capture_output=True)
            auth.chmod(0o600)
            for role,entry in catalog['components'].items():
                target=f'docker://{REPOSITORY}/{role}:v{catalog["release"]}'
                previous=subprocess.run(['skopeo','inspect','--authfile',str(auth),'--raw',target],capture_output=True,timeout=60)
                if previous.returncode==0:
                    assert 'sha256:'+hashlib.sha256(previous.stdout).hexdigest()==entry['digest'],'refusing to overwrite an existing release image'
                else:
                    assert any(code in previous.stderr.lower() for code in [b'manifest unknown',b'name unknown',b'not found']), 'could not safely check existing image tag: '+previous.stderr.decode()[-500:]
                    subprocess.run(['skopeo','copy','--authfile',str(auth),'--all','--preserve-digests','oci:'+str((a.images/role).resolve())+':v'+catalog['release'],target],check=True,timeout=900)
                print('Published immutable component: '+role,flush=True)
    deadline=time.monotonic()+a.public_wait
    while True:
        try:verify(catalog);break
        except (urllib.error.HTTPError,urllib.error.URLError) as error:
            if time.monotonic()>=deadline:raise
            print('Waiting for anonymous GHCR access; all eight packages must be public ('+str(getattr(error,'code','network'))+').',flush=True)
            time.sleep(15)


if __name__=='__main__':main()

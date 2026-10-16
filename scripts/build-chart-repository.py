#!/usr/bin/env python3
"""Build the Pages Helm repository from checksum-verified public release charts."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import tarfile
import io
import urllib.request

REPOSITORY='inferal-oss/kimage'


def metadata(path):
    headers={'User-Agent':'kimage-chart-repository'}
    if os.environ.get('GH_TOKEN'):headers['Authorization']='Bearer '+os.environ['GH_TOKEN']
    with urllib.request.urlopen(urllib.request.Request('https://api.github.com/repos/'+REPOSITORY+'/'+path,headers=headers),timeout=30) as response:return json.load(response)


def checked_chart(release,asset):
    version=release['tag_name'].removeprefix('v')
    assert re.fullmatch(r'\d+\.\d+\.\d+(?:-[A-Za-z0-9.-]+)?',version)
    name=f'kimage-{version}.tgz';assert asset['name']==name and asset['state']=='uploaded'
    url=f'https://github.com/{REPOSITORY}/releases/download/v{version}/{name}'
    assert asset['browser_download_url']==url and 0<asset['size']<=2*1024*1024
    with urllib.request.urlopen(url,timeout=60) as response:data=response.read(2*1024*1024+1)
    assert len(data)==asset['size'] and 'sha256:'+hashlib.sha256(data).hexdigest()==asset['digest'],'release chart SHA-256 mismatch'
    with tarfile.open(fileobj=io.BytesIO(data),mode='r:gz') as tar:
        manifest=tar.getmember('kimage/Chart.yaml');assert manifest.isfile() and manifest.size<16384
        text=tar.extractfile(manifest).read().decode()
        assert re.search(r'^version: '+re.escape(version)+r'$',text,re.M),'chart version mismatch'
        assert all(member.isfile() and member.name.startswith('kimage/') and '..' not in member.name.split('/') and member.size<1024*1024 for member in tar),'unsafe chart archive'
    return version,data


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--out',type=Path,default=Path('site/charts'));a=p.parse_args()
    entries=[];seen=set();a.out.mkdir(parents=True,exist_ok=True)
    for page in range(1,11):
        releases=metadata(f'releases?per_page=100&page={page}')
        for release in releases:
            if release['draft']:continue
            assets=[v for v in release['assets'] if re.fullmatch(r'kimage-\d+\.\d+\.\d+(?:-[A-Za-z0-9.-]+)?\.tgz',v['name'])]
            assert len(assets)<=1
            for asset in assets:
                version,data=checked_chart(release,asset);assert version not in seen;seen.add(version)
                (a.out/asset['name']).write_bytes(data)
                entries.append(dict(apiVersion='v2',name='kimage',description='Kubernetes-native image catalog, builder and delivery system',type='application',version=version,appVersion=version,home='https://kimage.dev',sources=['https://github.com/'+REPOSITORY],digest=hashlib.sha256(data).hexdigest(),created=release['published_at'],urls=['https://kimage.dev/charts/'+asset['name']]))
        if len(releases)<100:break
    else:raise RuntimeError('release listing exceeded limit')
    assert entries,'no public charts; refusing to publish an empty Helm repository'
    # Helm parses JSON as YAML. Creation times come from immutable releases.
    index=dict(apiVersion='v1',entries=dict(kimage=sorted(entries,key=lambda v:tuple(int(x) for x in v['version'].split('-')[0].split('.')),reverse=True)))
    (a.out/'index.yaml').write_text(json.dumps(index,indent=2)+'\n')
    print('Prepared verified Helm repository with '+str(len(entries))+' release(s).')


if __name__=='__main__':main()

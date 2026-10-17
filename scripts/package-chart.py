#!/usr/bin/env python3
"""Package the chart with the exact release's verified multi-platform image pins."""
import argparse
import gzip
import json
from pathlib import Path
import re
import tarfile
import io

ROOT=Path(__file__).resolve().parents[1]
ROLES={'admission','builder','exporter','gateway','input','operator','proxy','registry'}


def package(catalog_path,out,chart=None):
    catalog=json.loads(catalog_path.read_bytes());version=catalog['release']
    assert re.fullmatch(r'\d+\.\d+\.\d+(?:-[A-Za-z0-9.-]+)?',version)
    assert catalog['schema']==1 and isinstance(catalog['apiSchemaRevision'],int) and catalog['apiSchemaRevision']>=6 and set(catalog['components'])==ROLES
    assert catalog['repository']=='ghcr.io/inferal-oss/kimage','official chart must use official release images'
    for entry in catalog['components'].values():
        assert re.fullmatch(r'sha256:[0-9a-f]{64}',entry['digest'])
        assert entry['platforms']==[dict(os='linux',architecture=a) for a in ['amd64','arm64']]
    chart=chart or ROOT/'charts/kimage';manifest=(chart/'Chart.yaml').read_text()
    assert re.search(r'^version: '+re.escape(version)+r'$',manifest,re.M)
    assert re.search(r'^appVersion: "?'+re.escape(version)+r'"?$',manifest,re.M)
    declared=re.search(r'\"apiSchemaRevision\" ([0-9]+)',(chart/'templates/gitops.yaml').read_text());assert declared and int(declared[1])==catalog['apiSchemaRevision'], 'chart and catalog schema mismatch'
    files={str(p.relative_to(chart)):p.read_bytes() for p in chart.rglob('*') if p.is_file()}
    values=files['values.yaml'].decode()
    for role,entry in catalog['components'].items():
        values,count=re.subn(r'(?m)^(    '+role+r':) ""$',r'\1 "'+entry['digest']+'"',values)
        assert count==1,'missing source digest placeholder'
    files['values.yaml']=values.encode();files['LICENSE']=(ROOT/'LICENSE').read_bytes()
    out.mkdir(parents=True,exist_ok=True);target=out/f'kimage-{version}.tgz'
    with target.open('wb') as file,gzip.GzipFile(filename='',fileobj=file,mode='wb',mtime=0) as gz,tarfile.open(fileobj=gz,mode='w|',format=tarfile.USTAR_FORMAT) as tar:
        for name,data in sorted(files.items()):
            info=tarfile.TarInfo('kimage/'+name);info.mode=0o644;info.size=len(data);info.mtime=0;tar.addfile(info,io.BytesIO(data))
    print('Packaged '+target.name+' with eight complete multi-platform image pins')
    return target


if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--catalog',type=Path,required=True);p.add_argument('--out',type=Path,required=True)
    p.add_argument('--chart',type=Path)
    a=p.parse_args();package(a.catalog,a.out,a.chart)

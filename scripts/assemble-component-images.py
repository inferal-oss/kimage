#!/usr/bin/env python3
"""Assemble complete native graphs into deterministic multi-platform OCI indexes."""
import argparse
import gzip
import hashlib
import io
import importlib.util
import json
from pathlib import Path
import re
import stat
import tarfile

ROOT=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location('image_pack',ROOT/'scripts/package-component-images.py')
images=importlib.util.module_from_spec(spec);spec.loader.exec_module(images)
oci=images.oci
INDEX='application/vnd.oci.image.index.v1+json'


def checked_blob(blobs,descriptor):
    digest=descriptor['digest']
    assert re.fullmatch(r'sha256:[0-9a-f]{64}',digest),'invalid OCI digest'
    data=blobs[digest]
    assert len(data)==descriptor['size'] and oci.digest(data)==digest,'OCI descriptor mismatch'
    return data


def native_blobs(directory,entry,platform,role):
    filename=entry['archive']['file']
    assert filename==Path(filename).name and filename.endswith('.oci.tar'),'invalid native archive path'
    archive=directory/filename
    assert stat.S_ISREG(archive.lstat().st_mode),'native archive must be a regular file'
    assert oci.entry(archive)==entry['archive'],'native archive checksum mismatch'
    blobs={};seen=set();layout=None;index=None
    with tarfile.open(archive,'r:') as t:
        for member in t:
            assert member.isfile() and member.name not in seen and member.size<=512*1024*1024,'unsafe OCI archive member'
            seen.add(member.name);assert len(seen)<=256,'too many OCI members'
            data=t.extractfile(member).read()
            if member.name=='oci-layout':layout=json.loads(data)
            elif member.name=='index.json':index=json.loads(data)
            else:
                assert re.fullmatch(r'blobs/sha256/[0-9a-f]{64}',member.name),'unsafe OCI path'
                digest='sha256:'+member.name.rsplit('/',1)[1]
                assert oci.digest(data)==digest,'corrupt OCI blob'
                blobs[digest]=data
    assert layout=={'imageLayoutVersion':'1.0.0'} and index['manifests']==[entry['root']],'unexpected native OCI layout'
    manifest=json.loads(checked_blob(blobs,entry['root']))
    assert manifest['mediaType']==oci.MANIFEST
    config=json.loads(checked_blob(blobs,manifest['config']))
    assert config['os']==platform['os'] and config['architecture']==platform['architecture'],'native platform mismatch'
    assert config['config']['Entrypoint']==['/kimage-'+role], 'incorrect component entrypoint'
    assert config['config']['Labels']['org.opencontainers.image.source']=='https://github.com/inferal-oss/kimage'
    assert config['config']['Labels']['org.opencontainers.image.version']==entry['version']
    diff_ids=[]
    for layer in manifest['layers']:
        stored=checked_blob(blobs,layer)
        assert layer['mediaType'] in [oci.LAYER,oci.LAYER+'+gzip']
        if layer['mediaType'].endswith('+gzip'):
            with gzip.GzipFile(fileobj=io.BytesIO(stored)) as compressed:
                raw=compressed.read(512*1024*1024+1)
        else:raw=stored
        assert len(raw)<=512*1024*1024,'oversized image layer'
        diff_ids.append(oci.digest(raw))
    assert config['rootfs']==dict(type='layers',diff_ids=diff_ids),'OCI layer diff-id mismatch'
    return blobs


def assemble(directories,out,repository):
    assert re.fullmatch(r'[a-z0-9.-]+(?::[0-9]+)?/[a-z0-9._/-]+',repository),'invalid image repository'
    assert not any(part in ['.','..',''] for part in repository.split('/'))
    sources={}
    for directory in directories:
        native=json.loads((directory/'native-images.json').read_text())
        platform=native['platform'];arch=platform['architecture']
        assert native['schema']==1 and platform['os']=='linux' and arch in ['amd64','arm64'] and arch not in sources
        assert set(native['components'])==set(images.ROLES),'incomplete native image set'
        sources[arch]=(directory,native)
    assert set(sources)=={'amd64','arm64'},'both real native platforms are required'
    releases={v[1]['release'] for v in sources.values()};assert len(releases)==1,'native release mismatch'
    version=releases.pop()
    out.mkdir(parents=True,exist_ok=False)
    catalog=dict(schema=1,release=version,apiSchemaRevision=5,repository=repository,components={})
    for role in images.ROLES:
        blobs={};children=[];versions=set()
        for arch in ['amd64','arm64']:
            directory,native=sources[arch];entry=native['components'][role]
            versions.add(entry['version'])
            for digest,data in native_blobs(directory,entry,native['platform'],role).items():
                assert digest not in blobs or blobs[digest]==data,'conflicting OCI blob'
                blobs[digest]=data
            children.append(dict(entry['root'],platform=native['platform']))
        assert len(versions)==1,'native component versions differ'
        component_version=versions.pop()
        expected='0.3.0' if role in ['gateway','proxy'] else '3.1.2' if role=='registry' else '0.33.0' if role=='builder' else version
        assert component_version==expected,'unexpected component version'
        index=oci.encode(dict(schemaVersion=2,mediaType=INDEX,manifests=children,annotations={
            'org.opencontainers.image.source':'https://github.com/inferal-oss/kimage',
            'org.opencontainers.image.version':component_version}))
        root=oci.descriptor(index,INDEX);blobs[root['digest']]=index
        layout=out/role;(layout/'blobs/sha256').mkdir(parents=True)
        (layout/'oci-layout').write_bytes(oci.encode(dict(imageLayoutVersion='1.0.0')))
        (layout/'index.json').write_bytes(oci.encode(dict(schemaVersion=2,manifests=[dict(root,annotations={'org.opencontainers.image.ref.name':'v'+version})])))
        for digest,data in blobs.items():(layout/'blobs/sha256'/digest[7:]).write_bytes(data)
        (out/(role+'.root.json')).write_bytes(oci.encode(root))
        with tarfile.open(out/(role+'.oci.tar'),'w') as t:
            for file in sorted(layout.rglob('*')):
                if file.is_file():oci.append(t,str(file.relative_to(layout)),file.read_bytes())
        catalog['components'][role]=dict(version=component_version,digest=root['digest'],platforms=[dict(os='linux',architecture=arch) for arch in ['amd64','arm64']])
    (out/'component-images.json').write_bytes(oci.encode(catalog))
    print('Assembled verified amd64+arm64 component indexes for '+version)
    return catalog


if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--native',type=Path,action='append',required=True)
    p.add_argument('--out',type=Path,required=True)
    p.add_argument('--repository',default='ghcr.io/inferal-oss/kimage')
    a=p.parse_args();assemble(a.native,a.out,a.repository)

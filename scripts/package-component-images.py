#!/usr/bin/env python3
"""Build native OCI component images for standard registry publication."""
import argparse
import importlib.util
import json
from pathlib import Path
import re
import stat

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('oci_pack', ROOT/'scripts/package-bundle.py')
oci = importlib.util.module_from_spec(spec)
spec.loader.exec_module(oci)
ROLES = ['admission', 'builder', 'exporter', 'gateway', 'input', 'operator', 'proxy', 'registry']


def notices(metadata=None):
    text = ('kimage: Apache-2.0\nhttps://github.com/inferal-oss/kimage\n\n'
        'Distribution 3.1.2: Apache-2.0\nhttps://github.com/distribution/distribution\n\n'
        'BuildKit 0.33.0: Apache-2.0\nhttps://github.com/moby/buildkit\n\n'
        'runc: Apache-2.0\nhttps://github.com/opencontainers/runc\n')
    if metadata:
        packages = json.loads(Path(metadata).read_text())['packages']
        for package in sorted(packages, key=lambda p:(p['name'],p['version'])):
            text += '\n'+package['name']+' '+package['version']+': '+str(package['license'])+'\n'
            location = Path(package['manifest_path']).parent.resolve()
            for candidate in sorted(location.iterdir()):
                if not candidate.name.upper().startswith(('LICENSE','LICENCE','COPYING','NOTICE','COPYRIGHT')):
                    continue
                if stat.S_ISREG(candidate.lstat().st_mode) and candidate.resolve().is_relative_to(location):
                    assert candidate.stat().st_size <= 1024*1024, 'oversized dependency license'
                    text += '\n'+candidate.name+'\n'+candidate.read_text(errors='replace')+'\n'
    return text.encode()


def package(linux, registry, buildkit, out, architecture, version, metadata=None, api_schema_revision=6):
    assert re.fullmatch(r'\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?',version)
    assert architecture in ['amd64','arm64']
    assert isinstance(api_schema_revision,int) and 6<=api_schema_revision<=65535
    out.mkdir(parents=True,exist_ok=False)
    license_files={'LICENSE':(ROOT/'LICENSE').read_bytes(),'THIRD-PARTY-NOTICES.txt':notices(metadata)}
    components={}
    for role in ROLES:
        component_version='0.3.0' if role in ['gateway','proxy'] else '3.1.2' if role=='registry' else '0.33.0' if role=='builder' else version
        labels={'org.opencontainers.image.source':'https://github.com/inferal-oss/kimage',
            'org.opencontainers.image.licenses':'Apache-2.0','org.opencontainers.image.version':component_version,
            'org.opencontainers.image.title':'kimage '+role}
        archive=out/(role+'.oci.tar')
        if role=='builder':
            files=dict(license_files)
            files.update({('kimage-builder' if name=='buildkitd' else name):oci.static_elf(buildkit/name,architecture) for name in ['buildkitd','buildctl','buildkit-runc']})
            root=oci.image_files(files,archive,'kimage-builder',architecture=architecture,labels=labels,compress=True)
        else:
            source=registry if role=='registry' else linux/('kimage-'+role)
            root=oci.image(source,archive,'kimage-'+role,architecture=architecture,labels=labels,compress=True,additional=license_files)
        (out/(role+'.root.json')).write_bytes(oci.encode(root))
        components[role]=dict(version=component_version,archive=oci.entry(archive),root=root)
    native=dict(schema=1,apiSchemaRevision=api_schema_revision,release=version,platform=dict(os='linux',architecture=architecture),components=components)
    (out/'native-images.json').write_bytes(oci.encode(native))
    print('Packaged native '+architecture+' component images for '+version)
    return native


if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--linux-dir',type=Path,required=True)
    p.add_argument('--registry',type=Path,required=True)
    p.add_argument('--buildkit-dir',type=Path,required=True)
    p.add_argument('--out',type=Path,required=True)
    p.add_argument('--architecture',choices=['amd64','arm64'],required=True)
    p.add_argument('--version')
    p.add_argument('--api-schema-revision',type=int,default=6)
    p.add_argument('--cargo-metadata',type=Path)
    a=p.parse_args()
    version=a.version or re.search(r'(?ms)^\[workspace\.package\]\s*\n.*?^version\s*=\s*"([^"]+)"',(ROOT/'Cargo.toml').read_text())[1]
    package(a.linux_dir,a.registry,a.buildkit_dir,a.out,a.architecture,version,a.cargo_metadata,a.api_schema_revision)

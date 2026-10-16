#!/usr/bin/env python3
"""Assemble small installation assets. Component images are published separately to GHCR."""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import re
import shutil
import tempfile

ROOT=Path(__file__).resolve().parents[1]


def package(a):
    catalog=json.loads(a.catalog.read_bytes());v=catalog['release'];assert a.chart.name==f'kimage-{v}.tgz'
    workspace=re.search(r'(?ms)^\[workspace\.package\]\s*\n.*?^version\s*=\s*"([^"]+)"',(ROOT/'Cargo.toml').read_text())[1]
    assert v==workspace,'catalog release must match the source version'
    spec=importlib.util.spec_from_file_location('images',ROOT/'scripts/package-component-images.py');images=importlib.util.module_from_spec(spec);spec.loader.exec_module(images)
    spec=importlib.util.spec_from_file_location('chart',ROOT/'scripts/package-chart.py');chart=importlib.util.module_from_spec(spec);spec.loader.exec_module(chart)
    with tempfile.TemporaryDirectory(prefix='kimage-chart-verification-') as temp:
        expected=chart.package(a.catalog,Path(temp))
        assert expected.read_bytes()==a.chart.read_bytes(),'release chart does not match source and catalog pins'
    for platform in ['linux-amd64','linux-arm64','darwin-amd64','darwin-arm64']:
        source=a.installers/platform/'kimage-install';assert source.is_file() and not source.is_symlink()
        arch=platform.split('-')[1]
        if platform.startswith('linux-'):images.oci.static_elf(source,arch)
        else:
            header=source.read_bytes()[:16]
            assert len(header)==16 and header[:4]==bytes([207,250,237,254]) and int.from_bytes(header[4:8],'little')==0x1000000+({'amd64':7,'arm64':12}[arch]) and header[12:]==bytes([2,0,0,0]),'installer Mach-O architecture mismatch'
    a.out.mkdir(parents=True,exist_ok=False)
    for platform in ['linux-amd64','linux-arm64','darwin-amd64','darwin-arm64']:
        source=a.installers/platform/'kimage-install';assert source.is_file() and not source.is_symlink()
        target=a.out/f'kimage-v{v}-{platform}';shutil.copyfile(source,target);target.chmod(0o755)
    shutil.copyfile(a.catalog,a.out/'component-images.json');shutil.copyfile(a.chart,a.out/a.chart.name);shutil.copyfile(ROOT/'LICENSE',a.out/'LICENSE')
    (a.out/'THIRD-PARTY-NOTICES.txt').write_bytes(images.notices(a.metadata))
    (a.out/'SHA256SUMS').write_text(''.join(hashlib.sha256(p.read_bytes()).hexdigest()+'  '+p.name+'\n' for p in sorted(a.out.iterdir())))
    section=re.search(r'(?ms)^## \['+re.escape(v)+r'\][^\n]*\n(.*?)(?=^## |^\[Unreleased\]:|\Z)',(ROOT/'CHANGELOG.md').read_text());assert section
    (a.out/'RELEASE_NOTES.md').write_text(section[1].strip()+'\n\n[Install guide](https://kimage.dev/docs/#install) · [Helm repository](https://kimage.dev/charts/index.yaml) · [Agent essentials](https://kimage.dev/llms.txt)\n')
    print('Packaged four installers, small image catalog and pinned Helm chart for '+v)


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--catalog',type=Path,required=True);p.add_argument('--chart',type=Path,required=True);p.add_argument('--installers',type=Path,required=True);p.add_argument('--metadata',type=Path,required=True);p.add_argument('--out',type=Path,required=True);a=p.parse_args()
    package(a)


if __name__=='__main__':main()

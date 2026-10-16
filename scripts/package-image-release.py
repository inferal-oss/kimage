#!/usr/bin/env python3
"""Assemble small installation assets. Component images are published separately to GHCR."""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import re
import shutil

ROOT=Path(__file__).resolve().parents[1]


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--catalog',type=Path,required=True);p.add_argument('--chart',type=Path,required=True);p.add_argument('--installers',type=Path,required=True);p.add_argument('--metadata',type=Path,required=True);p.add_argument('--out',type=Path,required=True);a=p.parse_args()
    catalog=json.loads(a.catalog.read_bytes());v=catalog['release'];assert a.chart.name==f'kimage-{v}.tgz'
    a.out.mkdir(parents=True,exist_ok=False)
    for platform in ['linux-amd64','linux-arm64','darwin-amd64','darwin-arm64']:
        source=a.installers/platform/'kimage-install';assert source.is_file() and not source.is_symlink()
        target=a.out/f'kimage-v{v}-{platform}';shutil.copyfile(source,target);target.chmod(0o755)
    shutil.copyfile(a.catalog,a.out/'component-images.json');shutil.copyfile(a.chart,a.out/a.chart.name);shutil.copyfile(ROOT/'LICENSE',a.out/'LICENSE')
    spec=importlib.util.spec_from_file_location('images',ROOT/'scripts/package-component-images.py');images=importlib.util.module_from_spec(spec);spec.loader.exec_module(images)
    (a.out/'THIRD-PARTY-NOTICES.txt').write_bytes(images.notices(a.metadata))
    (a.out/'SHA256SUMS').write_text(''.join(hashlib.sha256(p.read_bytes()).hexdigest()+'  '+p.name+'\n' for p in sorted(a.out.iterdir())))
    section=re.search(r'(?ms)^## \['+re.escape(v)+r'\][^\n]*\n(.*?)(?=^## |^\[Unreleased\]:|\Z)',(ROOT/'CHANGELOG.md').read_text());assert section
    (a.out/'RELEASE_NOTES.md').write_text(section[1].strip()+'\n\n[Install guide](https://kimage.dev/docs/#install) · [Helm repository](https://kimage.dev/charts/index.yaml) · [Agent essentials](https://kimage.dev/llms.txt)\n')
    print('Packaged four installers, small image catalog and pinned Helm chart for '+v)


if __name__=='__main__':main()

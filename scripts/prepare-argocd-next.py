#!/usr/bin/env python3
"""Build-only future-version conformance fixture; never a public Cargo release.

This separate workspace declares exactly 0.5.1 <-> 0.5.2-argocd-test.0, keeps
0.5.1 templates and exercises the stable provisioner protocol using real binaries.
"""
import argparse
import json
from pathlib import Path
import shutil

ROOT=Path(__file__).resolve().parents[1]
VERSION='0.5.2-argocd-test.0'

def chart(destination):
    destination.mkdir(parents=True,exist_ok=True)
    source=ROOT/'charts/kimage'
    for name in ['Chart.yaml','values.yaml']:
        (destination/name).write_text((source/name).read_text().replace('0.5.1',VERSION))
    templates=destination/'templates';templates.mkdir(exist_ok=True)
    shutil.copyfile(source/'templates/gitops.yaml',templates/'gitops.yaml')

def prepare(destination):
    destination.mkdir(parents=True,exist_ok=False)
    for name in ['Cargo.toml','Cargo.lock','LICENSE']:
        shutil.copyfile(ROOT/name,destination/name)
    shutil.copytree(ROOT/'crates',destination/'crates')
    manifest=destination/'Cargo.toml';text=manifest.read_text()
    assert 'version = "0.5.1"' in text
    manifest.write_text(text.replace('version = "0.5.1"','version = "'+VERSION+'"').replace('version = "=0.5.1"','version = "='+VERSION+'"'))
    for manifest in (destination/'crates').glob('*/Cargo.toml'):
        manifest.write_text(manifest.read_text().replace('version = "=0.5.1"','version = "='+VERSION+'"'))
    # Share compiled dependencies without overwriting the real release's
    # binary paths or poisoning Cargo's cached output/fingerprint pairing.
    for role in ['operator','admission','input','exporter']:
        binary=destination/'crates/control/src/bin'/('kimage-'+role+'.rs')
        binary.rename(binary.with_name('kimage-'+role+'-argocd-conformance.rs'))
    lock=destination/'Cargo.lock';text=lock.read_text()
    for package in ['kimage-core','kimage-control','kimage-bootstrap']:
        old='name = "'+package+'"\nversion = "0.5.1"';assert old in text
        text=text.replace(old,'name = "'+package+'"\nversion = "'+VERSION+'"')
    lock.write_text(text)
    metadata=destination/'crates/control/src/release-compatibility.json'
    contract=json.loads(metadata.read_text());contract['release']=VERSION
    contract['transitions'].append(dict(**{'from':'0.5.1','to':VERSION},rollbackSafe=True))
    metadata.write_text(json.dumps(contract,indent=2)+'\n')
    # This fixture's resources are deliberately identical; freeze the returning
    # release just as a real subsequent release must freeze its predecessor.
    source=destination/'crates/control/src/pull_install/templates'
    frozen=source/'v0.5.1';frozen.mkdir()
    for path in source.glob('*.json'):shutil.copyfile(path,frozen/path.name)
    module=destination/'crates/control/src/pull_install/mod.rs';text=module.read_text()
    marker='    template(name)\n}'
    cases='\n'.join('            "'+p.name+'" => include_str!("templates/v0.5.1/'+p.name+'"),' for p in sorted(frozen.glob('*.json')))
    assert text.count(marker)==1
    module.write_text(text.replace(marker,'    if bindings.get("release").and_then(Value::as_str) == Some("0.5.1") {\n        return Ok(match name {\n'+cases+'\n            _ => anyhow::bail!("UnknownPredecessorTemplate"),\n        });\n    }\n'+marker))
    chart(destination/'charts/kimage')
    print('Prepared isolated future Argo conformance workspace '+VERSION)

if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--workspace',type=Path)
    parser.add_argument('--chart',type=Path)
    args=parser.parse_args();assert bool(args.workspace)!=bool(args.chart)
    if args.workspace:prepare(args.workspace)
    else:chart(args.chart)

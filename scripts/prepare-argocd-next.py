#!/usr/bin/env python3
"""Build-only future-version conformance fixture; never a public Cargo release.

This separate workspace declares exactly 0.6.1 <-> 0.6.2-argocd-test.0, keeps
0.6.1 templates and exercises the stable provisioner protocol using real binaries.
"""
import argparse
import json
from pathlib import Path
import shutil

ROOT=Path(__file__).resolve().parents[1]
VERSION='0.6.2-argocd-test.0'

def chart(destination):
    destination.mkdir(parents=True,exist_ok=True)
    source=ROOT/'charts/kimage'
    for name in ['Chart.yaml','values.yaml']:
        (destination/name).write_text((source/name).read_text().replace('0.6.1',VERSION))
    templates=destination/'templates';templates.mkdir(exist_ok=True)
    (templates/'gitops.yaml').write_text((source/'templates/gitops.yaml').read_text().replace('\"apiSchemaRevision\" 6','\"apiSchemaRevision\" 7'))

def prepare(destination):
    destination.mkdir(parents=True,exist_ok=False)
    for name in ['Cargo.toml','Cargo.lock','LICENSE']:
        shutil.copyfile(ROOT/name,destination/name)
    shutil.copytree(ROOT/'crates',destination/'crates')
    manifest=destination/'Cargo.toml';text=manifest.read_text()
    assert 'version = "0.6.1"' in text
    manifest.write_text(text.replace('version = "0.6.1"','version = "'+VERSION+'"').replace('version = "=0.6.1"','version = "='+VERSION+'"'))
    for manifest in (destination/'crates').glob('*/Cargo.toml'):
        manifest.write_text(manifest.read_text().replace('version = "=0.6.1"','version = "='+VERSION+'"'))
    # Share compiled dependencies without overwriting the real release's
    # binary paths or poisoning Cargo's cached output/fingerprint pairing.
    for role in ['operator','admission','input','exporter']:
        binary=destination/'crates/control/src/bin'/('kimage-'+role+'.rs')
        binary.rename(binary.with_name('kimage-'+role+'-argocd-conformance.rs'))
    (destination/'crates/control/src/bin/kimage-install-argocd-conformance.rs').write_text('include!("kimage-install.rs");\n')
    lock=destination/'Cargo.lock';text=lock.read_text()
    for package in ['kimage-core','kimage-control','kimage-bootstrap']:
        old='name = "'+package+'"\nversion = "0.6.1"';assert old in text
        text=text.replace(old,'name = "'+package+'"\nversion = "'+VERSION+'"')
    lock.write_text(text)
    metadata=destination/'crates/control/src/release-compatibility.json'
    contract=json.loads(metadata.read_text());contract['release']=VERSION;contract['formats']['apiSchemaRevision']=7;contract['apiReader']['capabilities'].append('future-notes-v1')
    contract['transitions'].append(dict(**{'from':'0.6.1','to':VERSION},rollbackSafe=True))
    metadata.write_text(json.dumps(contract,indent=2)+'\n')
    # This fixture's resources are deliberately identical; freeze the returning
    # release just as a real subsequent release must freeze its predecessor.
    source=destination/'crates/control/src/pull_install/templates'
    frozen=source/'v0.6.1';frozen.mkdir()
    for path in source.glob('*.json'):shutil.copyfile(path,frozen/path.name)
    module=destination/'crates/control/src/pull_install/mod.rs';text=module.read_text()
    marker='    template(name)\n}'
    cases='\n'.join('            "'+p.name+'" => include_str!("templates/v0.6.1/'+p.name+'"),' for p in sorted(frozen.glob('*.json')))
    assert text.count(marker)==1
    module.write_text(text.replace(marker,'    if bindings.get("release").and_then(Value::as_str) == Some("0.6.1") {\n        return Ok(match name {\n'+cases+'\n            _ => anyhow::bail!("UnknownPredecessorTemplate"),\n        });\n    }\n'+marker))
    # The future writer really has an additional schema and populated fields.
    # Returning binaries are the unchanged baseline artifacts, not rebuilt copies.
    schema=destination/'crates/control/src/schema.rs'
    text=schema.read_text().replace('pub const REVISION: u32 = 6;', 'pub const REVISION: u32 = 7;')
    text=text.replace('        if release {', '        if !release { spec["properties"]["futureNote"]["maxLength"]=json!(128); }\n        if release {',1)
    text=text.replace('            let provenance = &mut spec["properties"]["provenance"]["properties"];', '            let provenance = &mut spec["properties"]["provenance"]["properties"];\n            provenance["futureReceipt"]["maxLength"]=json!(128);',1)
    schema.write_text(text)
    api=destination/'crates/control/src/api.rs';text=api.read_text()
    text=text.replace('    /// Opaque additions survive older readers; they are never executed.', '    #[serde(skip_serializing_if = "Option::is_none")]\n    pub future_note: Option<String>,\n    /// Opaque additions survive older readers; they are never executed.',1)
    text=text.replace('            exports: Vec<crate::export_api::Target>,', '            exports: Vec<crate::export_api::Target>,\n            future_note: Option<String>,',1)
    text=text.replace('            exports: f.exports,','            exports: f.exports,\n            future_note: f.future_note,',1)
    text=text.replace('pub struct BuildProvenance {','pub struct BuildProvenance {\n    #[serde(skip_serializing_if = "Option::is_none")]\n    pub future_receipt: Option<String>,',1)
    api.write_text(text)
    for path in (destination/'crates/control/src').rglob('*.rs'):
        if path==api:continue
        text=path.read_text()
        text=text.replace('BuildProvenance {\n','BuildProvenance {\n            future_receipt: Some("future-notes-v1".into()),\n')
        path.write_text(text)
    compatibility=destination/'crates/control/src/api_compatibility.rs'
    text=compatibility.read_text().replace('&["oci-archive-v1", "containerfile-v1", "frozen-oci-inputs-v1"]','&["oci-archive-v1", "containerfile-v1", "frozen-oci-inputs-v1", "future-notes-v1"]')
    compatibility.write_text(text)
    chart(destination/'charts/kimage')
    print('Prepared isolated future Argo conformance workspace '+VERSION)

if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--workspace',type=Path)
    parser.add_argument('--chart',type=Path)
    args=parser.parse_args();assert bool(args.workspace)!=bool(args.chart)
    if args.workspace:prepare(args.workspace)
    else:chart(args.chart)

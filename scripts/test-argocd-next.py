#!/usr/bin/env python3
"""Ensure the future fixture tests actual versioned binaries without release drift."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

ROOT=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location('next_fixture',ROOT/'scripts/prepare-argocd-next.py')
fixture=importlib.util.module_from_spec(spec);spec.loader.exec_module(fixture)

class Fixture(unittest.TestCase):
    def test_tracked_future_chart_is_generated_from_current_provisioner(self):
        with tempfile.TemporaryDirectory() as directory:
            fixture.chart(Path(directory))
            expected={str(p.relative_to(ROOT/'charts/kimage')) for p in (ROOT/'charts/kimage').rglob('*') if p.is_file()}
            generated={str(p.relative_to(Path(directory))) for p in Path(directory).rglob('*') if p.is_file()}
            self.assertEqual(generated,expected)
            for name in sorted(expected):
                self.assertEqual((Path(directory)/name).read_bytes(),(ROOT/'tests/fixtures/argocd-next-chart'/name).read_bytes())
            for name in ['templates/gitops.yaml','templates/_environment.tpl']:
                self.assertIn('\"apiSchemaRevision\" 7',(Path(directory)/name).read_text())
    def test_future_contract_and_frozen_predecessor_are_isolated(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)/'source';fixture.prepare(root)
            self.assertIn('version = "'+fixture.VERSION+'"',(root/'Cargo.toml').read_text())
            self.assertIn('version = "='+fixture.VERSION+'"',(root/'crates/control/Cargo.toml').read_text())
            for role in ['operator','admission','input','exporter']:
                self.assertTrue((root/'crates/control/src/bin'/('kimage-'+role+'-argocd-conformance.rs')).is_file())
                self.assertFalse((root/'crates/control/src/bin'/('kimage-'+role+'.rs')).exists())
            contract=json.loads((root/'crates/control/src/release-compatibility.json').read_text())
            self.assertEqual(contract['release'],fixture.VERSION)
            self.assertEqual(contract['formats']['apiSchemaRevision'],7)
            self.assertIn('future_note: Option<String>',(root/'crates/control/src/api.rs').read_text())
            self.assertIn('future_receipt: Option<String>',(root/'crates/control/src/api.rs').read_text())
            self.assertIn('journal_format: Option<String>',(root/'crates/control/src/api.rs').read_text())
            self.assertIn('journal_format: Some("future-journal-v2".into())',(root/'crates/control/src/build_reconcile.rs').read_text())
            self.assertIn('pub const REVISION: u32 = 7;', (root/'crates/control/src/schema.rs').read_text())
            self.assertIn(dict(**{'from':'0.6.1','to':fixture.VERSION},rollbackSafe=True),contract['transitions'])
            self.assertEqual(json.loads((ROOT/'crates/control/src/release-compatibility.json').read_text())['release'],'0.6.1')
            templates=root/'crates/control/src/pull_install/templates'
            for source in templates.glob('*.json'):self.assertEqual(source.read_bytes(),(templates/'v0.6.1'/source.name).read_bytes())

if __name__=='__main__':unittest.main()

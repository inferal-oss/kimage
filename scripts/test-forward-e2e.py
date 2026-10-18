#!/usr/bin/env python3
"""Boundaries for opt-in forward-schema and empty-namespace acceptance."""
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import runpy
import subprocess
import sys
import tempfile
import unittest
ROOT=Path(__file__).resolve().parents[1]
def module(name):
    spec=importlib.util.spec_from_file_location(name,ROOT/'scripts'/(name+'.py'));value=importlib.util.module_from_spec(spec);spec.loader.exec_module(value);return value
forward=module('e2e-forward-compatibility');upgrade=module('e2e-upgrade');argo=module('e2e-argocd')
class Boundaries(unittest.TestCase):
    def test_future_schema_requires_opt_in_before_loading_config_and_disposable_api_before_client(self):
        result=subprocess.run([sys.executable,str(ROOT/'scripts/e2e-forward-compatibility.py'),'--config','/nonexistent'],capture_output=True,text=True)
        self.assertNotEqual(result.returncode,0);self.assertIn('Opt in with --run',result.stderr);self.assertNotIn('FileNotFoundError',result.stderr)
        with self.assertRaisesRegex(RuntimeError,'explicitly disposable'):forward.Forward({})
        with self.assertRaisesRegex(RuntimeError,'explicitly disposable'):forward.Forward(dict(disposableApi=False,kubeconfig='/nonexistent'))
    def test_argo_future_schema_guard_precedes_client_and_output_creation(self):
        config={k:'unused' for k in ['kubeconfig','context','namespace','systemNamespace','installation','probes','output','catalog','installer','scope','port','registryNode','runtimeProfile']}
        config['nextCatalog']='/nonexistent'
        for explicit in [None,False]:
            config['disposableApi']=explicit
            with self.assertRaisesRegex(RuntimeError,'explicitly disposable'):argo.Argo(config)
    def test_namespace_reuse_never_writes_when_uid_or_existing_workload_data_are_wrong(self):
        for manager in ['cli','argo']:
            for mismatch in ['uid','workload','scope']:
                class API:
                    def get(self,kind,name=None,namespace=None):
                        if kind=='namespace':return None if name=='fresh-system' else dict(metadata=dict(uid='foreign' if mismatch=='uid' else 'expected'))
                        return dict(items=[dict(metadata=dict(uid='foreign-data'))] if mismatch=='workload' and kind=='imagereleases.kimage.dev' else [])
                    def call(self,*args,**kwargs):raise AssertionError('must not mutate a nonempty or replaced application namespace')
                if manager=='cli':
                    fix=upgrade.Upgrade.__new__(upgrade.Upgrade);fix.app='existing';fix.system='fresh-system';fix.api=API();fix.report={};fix.config=dict(existingEmptyApplicationNamespaceUid='expected',scope='cluster' if mismatch=='scope' else 'namespace')
                    with self.assertRaisesRegex(RuntimeError,'UID or scope mismatch|contains workload data'):fix.preflight()
                else:
                    fix=argo.Argo.__new__(argo.Argo);fix.app='existing';fix.system='fresh-system';fix.api=API();fix.report={};fix.config=dict(existingEmptyApplicationNamespaceUid='expected',scope='cluster' if mismatch=='scope' else 'namespace')
                    with self.assertRaisesRegex(RuntimeError,'namespace-scoped|UID or fresh|contains workload data'):fix.prepare_namespaces()
    def test_forward_cleanup_uses_one_uid_inventory_and_refuses_replacements(self):
        class API:
            run_id='run'
            deployments=[('shared-consumer','owned'),('future-consumer','future-owned'),('shared-consumer','owned')]
            def __init__(self,replaced=False):self.objects={'shared-consumer':'replacement' if replaced else 'owned','future-consumer':'future-owned'};self.deleted=[];self.waited=False
            def get(self,kind,name=None,namespace=None,selector=None):
                if kind=='pods':return dict(items=[])
                return dict(metadata=dict(uid=self.objects[name])) if name in self.objects else None
            def call(self,args,body):
                name=args[2].rsplit('/',1)[-1];assert body['preconditions']['uid']==self.objects[name];self.deleted.append(name);del self.objects[name]
            def wait(self,predicate,message):assert predicate();self.waited=True
        fixture=forward.Forward.__new__(forward.Forward);fixture.app='apps';fixture.api=API();fixture.remove_consumers()
        self.assertEqual(fixture.api.deleted,['shared-consumer','future-consumer']);self.assertTrue(fixture.api.waited)
        fixture.api=API(replaced=True)
        with self.assertRaisesRegex(RuntimeError,'consumer identity changed'):fixture.remove_consumers()
        self.assertEqual(fixture.api.deleted,[])
    def test_pending_changelog_versions_keep_the_latest_published_comparison(self):
        source=(ROOT/'scripts/check-changelog.py').read_text();sha='a'*40
        notes='# Changelog\n\nhttps://keepachangelog.com/en/1.1.0/\n\n## [Unreleased]\n\n### Added\n\n- Foundation. Implementation pending commit.\n\n**Commits:** ['+sha[:7]+'](https://github.com/inferal-oss/kimage/commit/'+sha+').\n\n## [0.6.0] - 2026-10-17\n\n### Added\n\n- Previous release. Commits: ['+sha[:7]+'](https://github.com/inferal-oss/kimage/commit/'+sha+').\n\n**Commits:** ['+sha[:7]+'](https://github.com/inferal-oss/kimage/commit/'+sha+').\n\n[Unreleased]: https://github.com/inferal-oss/kimage/compare/v0.6.0...HEAD\n[0.6.0]: https://github.com/inferal-oss/kimage/compare/v0.5.2...v0.6.0\n'
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);(root/'scripts').mkdir();(root/'scripts/check-changelog.py').write_text(source);(root/'Cargo.toml').write_text('[workspace.package]\nversion = "0.6.1"\n');(root/'CHANGELOG.md').write_text(notes)
            with contextlib.redirect_stdout(io.StringIO()):runpy.run_path(str(root/'scripts/check-changelog.py'),run_name='__main__')
            (root/'CHANGELOG.md').write_text(notes.replace('compare/v0.6.0...HEAD','compare/v0.6.1...HEAD'))
            with self.assertRaises(AssertionError):runpy.run_path(str(root/'scripts/check-changelog.py'),run_name='__main__')
if __name__=='__main__':unittest.main()

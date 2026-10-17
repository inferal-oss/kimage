#!/usr/bin/env python3
"""Offline safety boundaries for the existing-cluster Argo CD harness."""
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT=Path(__file__).resolve().parents[1]
def module(name):
    spec=importlib.util.spec_from_file_location(name,ROOT/'scripts'/(name+'.py'));m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m);return m
argo=module('e2e-argocd');prepare=module('prepare-argocd-fixture')

class Boundaries(unittest.TestCase):
    def test_opt_in_precedes_configuration_access(self):
        result=subprocess.run([sys.executable,str(ROOT/'scripts/e2e-argocd.py'),'--config','/nonexistent'],text=True,capture_output=True)
        self.assertNotEqual(result.returncode,0);self.assertIn('Opt in with --run',result.stderr);self.assertNotIn('FileNotFoundError',result.stderr)
    def test_disposable_controller_install_cannot_run_on_a_real_cluster(self):
        with patch.dict(prepare.os.environ,{},clear=True),patch.object(prepare.urllib.request,'urlopen') as network,patch.object(prepare.subprocess,'run') as mutation:
            with self.assertRaisesRegex(AssertionError,'disposable GitHub-hosted'):prepare.main()
            network.assert_not_called();mutation.assert_not_called()
    def test_receipt_contains_only_secret_identity_and_hash(self):
        class API:
            def get(self,kind,name=None,namespace=None):
                if kind=='imagereleases.kimage.dev':return dict(items=[dict(metadata=dict(name='release',uid='release-uid'))])
                return dict(metadata=dict(uid=kind+'-uid'),data={'private-key':'private-secret-value'})
        fixture=argo.Argo.__new__(argo.Argo);fixture.api=API();fixture.id='fixture';fixture.app='app';fixture.system='system'
        output=io.StringIO()
        with contextlib.redirect_stdout(output):value=fixture.fingerprint()
        serialized=json.dumps(value)
        self.assertNotIn('private-key',serialized);self.assertNotIn('private-secret-value',serialized);self.assertEqual(output.getvalue(),'')
        self.assertTrue(value['secret/fixture-registry-credentials']['sha256'].startswith('sha256:'))
    def test_sync_termination_uses_terminating_never_fakes_success(self):
        fixture=argo.Argo.__new__(argo.Argo);fixture.id='isolated';calls=[]
        fixture.application_state=lambda:dict(status=dict(operationState=dict(phase='Running' if not calls else 'Failed')))
        fixture.patch=lambda *args:calls.append(args)
        class API:
            def wait(self,predicate,message):assert predicate()
        fixture.api=API();fixture.stop_sync()
        self.assertEqual(calls,[('applications.argoproj.io','isolated',dict(status=dict(operationState=dict(phase='Terminating'))),'argocd')])
    def test_unknown_placement_uses_one_consumer_per_available_architecture(self):
        for architectures in [{'amd64'},{'arm64'},{'amd64','arm64'}]:
            with patch.object(argo.mixed,'ARCHITECTURES',architectures):
                value=argo.mixed.deployment_body('fresh','apps')
                self.assertEqual(value['spec']['replicas'],len(architectures))
                self.assertNotIn('nodeSelector',value['spec']['template']['spec'])
    def test_source_replacement_removes_old_targets_and_policy_keys(self):
        fixture=argo.Argo.__new__(argo.Argo);fixture.id='isolated';calls=[]
        previous={'metadata':{'uid':'application-uid','resourceVersion':'17'},'spec':{'source':{'repoURL':'https://github.com/inferal-oss/kimage.git','path':'charts/kimage','targetRevision':'exact','helm':{'valuesObject':{'gitops':{'enabled':True,'targetCatalog':{'release':'unsupported'},'credentialSecrets':{'registry':'external'}},'nodePolicy':{'nodeSelector':{'old':'selected'},'workerPins':{'arm64':'old-node'}}}}}}}
        fixture.application_state=lambda:previous
        class API:
            def call(self,args):calls.append(args)
        fixture.api=API()
        result=fixture.desired(dict(gitops=dict(enabled=True),nodePolicy=dict(nodeSelector={},workerPins={})))
        args=calls[0];self.assertIn('--type=json',args)
        operations=json.loads(args[-1])
        self.assertEqual(operations[:2],[dict(op='test',path='/metadata/uid',value='application-uid'),dict(op='test',path='/metadata/resourceVersion',value='17')])
        self.assertEqual(operations[2],dict(op='replace',path='/spec/source',value=result))
        self.assertEqual(result['helm']['valuesObject']['gitops'],dict(enabled=True))
        self.assertEqual(result['helm']['valuesObject']['nodePolicy'],dict(nodeSelector={},workerPins={}))
        self.assertIn('targetCatalog',previous['spec']['source']['helm']['valuesObject']['gitops'])
    def test_configuration_requires_isolated_names_and_one_source(self):
        config=dict(kubeconfig='fixture',context='fixture',namespace='production',systemNamespace='kimage-system',installation='kimage',probes='fixture',output='fixture',catalog='fixture',installer='fixture',scope='cluster',port=17751,registryNode='fixture',runtimeProfile='inferal',sourceRevision='commit')
        with self.assertRaisesRegex(RuntimeError,'Isolated kimage fixture'):argo.Argo(config)
        config['chartRepository']='https://kimage.dev/charts'
        with self.assertRaisesRegex(RuntimeError,'exact Git source revision or public chart'):argo.Argo(config)

if __name__=='__main__':unittest.main()

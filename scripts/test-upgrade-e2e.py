#!/usr/bin/env python3
"""Offline boundaries for real-cluster acceptance and predecessor/candidate trust."""
import contextlib
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT=Path(__file__).resolve().parents[1]

def module(name):
    spec=importlib.util.spec_from_file_location(name,ROOT/'scripts'/(name+'.py'))
    value=importlib.util.module_from_spec(spec);spec.loader.exec_module(value);return value

fetch=module('fetch-predecessor')
publish=module('publish-component-images')
upgrade=module('e2e-upgrade')

class Boundaries(unittest.TestCase):
    def test_lifecycle_configuration_requires_distinct_exact_published_predecessor(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);previous=root/'previous';previous.mkdir()
            assets=[]
            for name in ['kimage-v0.6.0-linux-amd64','component-images.json','kimage-0.6.0.tgz']:
                data=(name+'-fixture').encode();(previous/name).write_bytes(data)
                assets.append(dict(name=name,size=len(data),sha256='sha256:'+hashlib.sha256(data).hexdigest()))
            (previous/'verified.json').write_text(json.dumps(dict(release='0.6.0',assets=assets)))
            catalog=root/'catalog.json';catalog.write_text(json.dumps(dict(release='0.6.1')))
            installer=root/'installer';installer.write_bytes(b'fixture')
            chart=root/'chart';chart.write_bytes(b'fixture')
            config=dict(kubeconfig='fixture',context='fixture',namespace='app',systemNamespace='system',installation='install',probes=str(root/'probes'),output=str(root/'output'),installer=str(installer),previous=str(previous),catalog=str(catalog),chart=str(chart),mode='cli',scope='namespace',runtimeProfile='k3s',port=17701)
            fixture=upgrade.Upgrade(config)
            self.assertEqual(fixture.source,'0.6.0');self.assertEqual(fixture.version,'0.6.1')
            (previous/'verified.json').write_text(json.dumps(dict(release='0.6.1',assets=assets)))
            config['output']=str(root/'wrong-output')
            with self.assertRaisesRegex(RuntimeError,'Published predecessor verification'):upgrade.Upgrade(config)

    def test_helm_reinstall_uses_target_mirror_and_pins_without_changing_predecessor_values(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);original=dict(scope='namespace',applicationNamespace='apps',runtimeProfile='k3s',registryNode='fixture',port=17702)
            path=root/'helm-values.json';path.write_text(json.dumps(original));before=path.read_bytes()
            fixture=upgrade.Upgrade.__new__(upgrade.Upgrade);fixture.config=dict(mode='helm',chart='target-chart');fixture.output=root;fixture.id='fixture';fixture.system='system';fixture.version='0.6.1'
            fixture.target=dict(release='0.6.1',repository='127.0.0.1:14901/kimage',components={r:dict(digest='sha256:'+str(i)*64) for i,r in enumerate(['operator','admission','input','exporter','builder','gateway','proxy','registry'])})
            calls=[]
            def command(args):
                self.assertEqual(args[:4],['helm','install','fixture','target-chart'])
                values=json.loads(Path(args[args.index('-f')+1]).read_text())
                self.assertEqual({k:v for k,v in values.items() if k!='images'},original)
                self.assertEqual(values['images'],dict(repository=fixture.target['repository'],digests={r:e['digest'] for r,e in fixture.target['components'].items()}))
                calls.append('install')
            fixture.common=lambda:[];fixture.run_command=command;fixture.assert_images=lambda version,catalog:calls.append((version,catalog));fixture.install_current()
            self.assertEqual(path.read_bytes(),before);self.assertEqual(calls,['install',('0.6.1',fixture.target)])

    def test_published_predecessor_is_drained_before_helm_can_delete_workers(self):
        import types
        events=[]
        class API:
            def get(self,kind,*args,**kwargs):
                if kind=='deployment':return dict(metadata=dict(uid='operator',resourceVersion='11',labels={'kimage.dev/installation':'fixture'},annotations={'kimage.dev/manager':'helm'}))
                assert events==['scale'];events.append('pods-gone');return dict(items=[])
            def call(self,args,**kwargs):
                patch=json.loads(args[-1]);assert patch==dict(metadata=dict(uid='operator',resourceVersion='11'),spec=dict(replicas=0))
                events.append('scale');return types.SimpleNamespace(returncode=0)
            def wait(self,predicate,message):assert predicate()
        fixture=upgrade.Upgrade.__new__(upgrade.Upgrade)
        fixture.api=API();fixture.source=fetch.VERSION;fixture.config=dict(mode='helm');fixture.system='system';fixture.id='fixture'
        fixture.architectures=set()
        fixture.check=lambda message:None
        def command(args):
            self.assertEqual(events,['scale','pods-gone']);self.assertEqual(args[:3],['helm','uninstall','fixture']);events.append('helm-uninstall')
        fixture.run_command=command
        fixture.uninstall_previous()
        self.assertEqual(events,['scale','pods-gone','helm-uninstall'])

    def test_predecessor_worker_retirement_checks_the_template_and_delete_preconditions(self):
        import copy,types
        expected=dict(metadata=dict(name='fixture-builder-arm64',labels={'kimage.dev/installation':'fixture'}),spec=dict(nodeName='native',hostNetwork=False,containers=[dict(name='builder',image='pinned',volumeMounts=[dict(name='cache',mountPath='/cache',readOnly=False)])],automountServiceAccountToken=False))
        for changed in ['none','defaults','image','owner','release','token','mount']:
            worker=copy.deepcopy(expected);worker['metadata'].update(uid='worker-uid',resourceVersion='17')
            if changed=='image':worker['spec']['containers'][0]['image']='foreign'
            if changed=='owner':worker['metadata']['ownerReferences']=[dict(apiVersion='v1',kind='ConfigMap',name='foreign',uid='foreign')]
            if changed=='release':worker['metadata']['annotations']={'meta.helm.sh/release-name':'foreign'}
            if changed=='defaults':
                del worker['spec']['hostNetwork'];del worker['spec']['containers'][0]['volumeMounts'][0]['readOnly']
            if changed=='token':del worker['spec']['automountServiceAccountToken']
            if changed=='mount':worker['spec']['containers'][0]['volumeMounts'][0]['readOnly']=True
            events=[]
            class API:
                def get(self,kind,*args):
                    if args and args[0]=='fixture-installation':return dict(metadata=dict(uid='state-uid',labels={'kimage.dev/installation':'fixture'}))
                    if kind=='configmap':return dict(metadata=dict(labels={'kimage.dev/installation':'fixture'}),data={'pod.json':json.dumps(expected)})
                    return None if events else worker
                def call(self,args,body,**kwargs):
                    self_body=dict(apiVersion='v1',kind='DeleteOptions',gracePeriodSeconds=1,preconditions=dict(uid='worker-uid',resourceVersion='17'))
                    assert body==self_body and args[2]=='/api/v1/namespaces/system/pods/fixture-builder-arm64'
                    events.append('delete');return types.SimpleNamespace(returncode=0)
                def wait(self,predicate,message):assert predicate()
            fixture=upgrade.Upgrade.__new__(upgrade.Upgrade);fixture.api=API();fixture.source=fetch.VERSION;fixture.system='system';fixture.id='fixture';fixture.check=lambda text:None
            if changed not in ('none','defaults'):
                with self.assertRaisesRegex(RuntimeError,'ownership or specification|foreign owner'):fixture.retire_previous_worker('arm64')
                self.assertEqual(events,[])
            else:
                fixture.retire_previous_worker('arm64');self.assertEqual(events,['delete'])

    def test_uninstall_timeout_inventory_excludes_secret_values(self):
        class API:
            def get(self,kind,**kwargs):
                return dict(items=[dict(metadata=dict(name='fixture-secret',ownerReferences=[]),data={'credential':'private'})] if kind=='secrets' else [])
            def wait(self,predicate,message):
                assert not predicate()
                raise TimeoutError('fixture')
        fixture=upgrade.Upgrade.__new__(upgrade.Upgrade)
        fixture.api=API();fixture.system='system';fixture.id='install'
        output=io.StringIO()
        with contextlib.redirect_stdout(output),self.assertRaisesRegex(RuntimeError,'fixture-secret') as error:fixture.assert_removed()
        self.assertNotIn('credential',str(error.exception))
        self.assertNotIn('private',str(error.exception))
        self.assertIn('fixture-secret',output.getvalue())
        self.assertNotIn('credential',output.getvalue())
        self.assertNotIn('private',output.getvalue())

    def test_fixture_preflight_creates_a_private_shared_minified_kubeconfig(self):
        import types
        node=dict(metadata=dict(name='native'),status=dict(nodeInfo=dict(operatingSystem='linux',architecture='amd64'),conditions=[dict(type='Ready',status='True')]))
        configuration=json.dumps({'current-context':'fixture','contexts':[],'clusters':[],'users':[]})
        class API:
            def get(self,kind,name=None):return dict(items=[node]) if kind=='nodes' else None
            def call(self,args,value=None):return types.SimpleNamespace(stdout=configuration)
            def create(self,value):return value
        with tempfile.TemporaryDirectory() as directory,patch.object(upgrade.mixed,'verify_probe',return_value=b'fixture-elf'),contextlib.redirect_stdout(io.StringIO()):
            fixture=upgrade.Upgrade.__new__(upgrade.Upgrade)
            fixture.config=dict(context='fixture',probes=directory)
            fixture.api=API();fixture.app='app';fixture.system='system';fixture.id='install'
            fixture.output=Path(directory);fixture.environment=dict(os.environ);fixture.report=dict(checks=[])
            fixture.private_config=None
            try:
                fixture.preflight()
                path=Path(fixture.private_config.name)
                self.assertEqual(path.stat().st_mode & 0o777,0o600)
                self.assertEqual(path.read_text(),configuration)
                self.assertEqual(fixture.environment['KUBECONFIG'],str(path))
                self.assertNotIn('KIMAGE_API_SERVER',fixture.environment)
                self.assertNotIn('KIMAGE_TLS_SERVER_NAME',fixture.environment)
            finally:
                if fixture.private_config:Path(fixture.private_config.name).unlink(missing_ok=True)

    def test_real_cluster_acceptance_requires_opt_in_before_opening_configuration(self):
        result=subprocess.run([sys.executable,str(ROOT/'scripts/e2e-upgrade.py'),'--config','/nonexistent'],text=True,capture_output=True)
        self.assertNotEqual(result.returncode,0)
        self.assertIn('Opt in with --run',result.stderr)
        self.assertNotIn('FileNotFoundError',result.stderr)

    def test_published_predecessor_digest_size_and_metadata_only_authentication(self):
        names=['kimage-v0.6.0-linux-amd64','component-images.json','kimage-0.6.0.tgz']
        bodies={name:(name+'-published').encode() for name in names}
        assets=[dict(name=name,state='uploaded',size=len(bodies[name]),digest='sha256:'+hashlib.sha256(bodies[name]).hexdigest(),browser_download_url=fetch.ASSETS+name) for name in names]
        release=dict(tag_name='v0.6.0',draft=False,prerelease=False,assets=assets)
        class Metadata:
            def open(self,request,timeout):
                assert request.full_url==fetch.API and request.get_header('Authorization')=='Bearer fixture-token'
                return io.BytesIO(json.dumps(release).encode())
        def asset(request,timeout):
            self.assertIsNone(request.get_header('Authorization'))
            self.assertTrue(request.full_url.startswith(fetch.ASSETS))
            return io.BytesIO(bodies[request.full_url.removeprefix(fetch.ASSETS)])
        with tempfile.TemporaryDirectory() as directory,patch.dict(os.environ,{'KIMAGE_GITHUB_TOKEN':'fixture-token'}),patch.object(fetch.urllib.request,'build_opener',return_value=Metadata()),patch.object(fetch.urllib.request,'urlopen',side_effect=asset),contextlib.redirect_stdout(io.StringIO()):
            result=fetch.fetch('amd64',Path(directory))
            self.assertEqual(len(result),3)
            for item in result:self.assertEqual(hashlib.sha256((Path(directory)/item['name']).read_bytes()).hexdigest(),item['sha256'][7:])
            release['assets'][0]['digest']='sha256:'+'0'*64
            with self.assertRaises(AssertionError):fetch.fetch('amd64',Path(directory)/'changed')

    def test_metadata_redirects_and_release_named_candidate_tags_are_refused(self):
        with self.assertRaisesRegex(RuntimeError,'redirect refused'):fetch.NoMetadataRedirect().redirect_request(None,None,None,None,None,None)
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory);(path/'component-images.json').write_text(json.dumps(dict(repository=publish.REPOSITORY,release='0.5.0',components={})))
            with patch.object(sys,'argv',['publish-component-images.py','--images',str(path),'--candidate-tag','v0.5.0']),patch.object(publish.subprocess,'run') as run:
                with self.assertRaisesRegex(AssertionError,'candidate tag'):publish.main()
                run.assert_not_called()

if __name__=='__main__':unittest.main()

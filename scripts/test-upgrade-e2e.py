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

class Boundaries(unittest.TestCase):
    def test_real_cluster_acceptance_requires_opt_in_before_opening_configuration(self):
        result=subprocess.run([sys.executable,str(ROOT/'scripts/e2e-upgrade.py'),'--config','/nonexistent'],text=True,capture_output=True)
        self.assertNotEqual(result.returncode,0)
        self.assertIn('Opt in with --run',result.stderr)
        self.assertNotIn('FileNotFoundError',result.stderr)

    def test_published_predecessor_digest_size_and_metadata_only_authentication(self):
        names=['kimage-v0.4.1-linux-amd64','component-images.json','kimage-0.4.1.tgz']
        bodies={name:(name+'-published').encode() for name in names}
        assets=[dict(name=name,state='uploaded',size=len(bodies[name]),digest='sha256:'+hashlib.sha256(bodies[name]).hexdigest(),browser_download_url=fetch.ASSETS+name) for name in names]
        release=dict(tag_name='v0.4.1',draft=False,prerelease=False,assets=assets)
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

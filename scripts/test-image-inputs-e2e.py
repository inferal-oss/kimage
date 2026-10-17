#!/usr/bin/env python3
"""Frozen-input failure receipts expose codes; optional logs remain ciphertext."""
import contextlib
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

spec=importlib.util.spec_from_file_location('inputs',Path(__file__).with_name('e2e-image-inputs.py'))
inputs=importlib.util.module_from_spec(spec);spec.loader.exec_module(inputs)

class Boundaries(unittest.TestCase):
    def test_failure_status_masks_messages_and_private_logs_only_leave_as_ciphertext(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary);key=root/'key.pem';cert=root/'cert.pem'
            subprocess.run(['openssl','req','-x509','-newkey','rsa:2048','-nodes','-keyout',str(key),'-out',str(cert),'-days','1','-subj','/CN=kimage-test'],capture_output=True,check=True)
            binary=root/'installer';binary.write_text('#!'+sys.executable+'\nprint("PRIVATE_BUILD_OUTPUT")\n');binary.chmod(0o700)
            class API:
                namespace='apps';config=dict(kubeconfig=str(root/'config'))
                def get(self,*args,**kwargs):
                    return dict(items=[dict(metadata=dict(name=name,uid=name+'-uid'),status=dict(conditions=[dict(reason=reason,message='PRIVATE_MESSAGE')])) for name,reason in [('fixture-owned','ContainerfileSolveFailed'),('fixture-tampered','PRIVATE_REASON'),('foreign','ContainerfileSolveFailed')]])
            fixture=inputs.ImageInputs.__new__(inputs.ImageInputs)
            fixture.api=API();fixture.output=root;fixture.prefix='fixture';fixture.report={};fixture.installer=binary;fixture.active_image='fixture-owned';fixture.architectures={'amd64','arm64'}
            public=io.StringIO()
            with patch.dict(os.environ,KIMAGE_TEST_DIAGNOSTIC_CERT=cert.read_text()),contextlib.redirect_stdout(public):fixture.failure_diagnostics()
            self.assertNotIn('PRIVATE',public.getvalue())
            self.assertEqual(fixture.report['failureStatus'],[dict(image='fixture-owned',reasons=['ContainerfileSolveFailed']),dict(image='fixture-tampered',reasons=['unexpected'])])
            encrypted=root/'failure.p7m';self.assertNotIn(b'PRIVATE',encrypted.read_bytes());self.assertEqual(encrypted.stat().st_mode&0o777,0o600)
            self.assertFalse((root/'recipient.pem').exists())
            result=subprocess.run(['openssl','cms','-decrypt','-binary','-inform','DER','-in',str(encrypted),'-inkey',str(key),'-recip',str(cert)],capture_output=True,check=True)
            self.assertTrue(all(v['stdout']=='PRIVATE_BUILD_OUTPUT\n' for v in json.loads(result.stdout).values()))

if __name__=='__main__':unittest.main()

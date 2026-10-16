#!/usr/bin/env python3
"""Verify optional failure artifacts contain ciphertext and keep Helm output private."""
import json,os,subprocess,sys,tempfile,unittest
from pathlib import Path
SCRIPT=Path(__file__).with_name('helm-diagnostics.py').resolve()
class Diagnostics(unittest.TestCase):
    def test_failure_encrypts_both_streams_and_success_passes_through(self):
        with tempfile.TemporaryDirectory() as temp:
            root=Path(temp);key=root/'key.pem';cert=root/'cert.pem'
            subprocess.run(['openssl','req','-x509','-newkey','rsa:2048','-nodes','-keyout',str(key),'-out',str(cert),'-days','1','-subj','/CN=kimage-test'],check=True,capture_output=True)
            helm=root/'real-helm';helm.write_text('#!'+sys.executable+'\nimport sys\nprint("PRIVATE_STDOUT" if sys.argv[1]=="upgrade" else "OK")\nprint("PRIVATE_STDERR" if sys.argv[1]=="upgrade" else "WARNING",file=sys.stderr)\nraise SystemExit(9 if sys.argv[1]=="upgrade" else 0)\n');helm.chmod(0o700)
            directory=root/'capture'
            subprocess.run([sys.executable,str(SCRIPT),'--setup',str(directory),'--helm',str(helm)],env=dict(os.environ,KIMAGE_HELM_DIAGNOSTIC_CERT=cert.read_text()),check=True,capture_output=True)
            failed=subprocess.run([str(directory/'bin/helm'),'upgrade','fixture'],capture_output=True)
            self.assertEqual(failed.returncode,9);self.assertNotIn(b'PRIVATE_',failed.stdout+failed.stderr)
            encrypted=list((directory/'output').iterdir());self.assertEqual(len(encrypted),1)
            self.assertNotIn(b'PRIVATE_',encrypted[0].read_bytes());self.assertEqual(encrypted[0].stat().st_mode&0o777,0o600)
            decoded=subprocess.run(['openssl','cms','-decrypt','-binary','-inform','DER','-in',str(encrypted[0]),'-inkey',str(key),'-recip',str(cert)],check=True,capture_output=True)
            payload=json.loads(decoded.stdout);self.assertEqual(payload['stderr'],'PRIVATE_STDERR\n');self.assertEqual(payload['stdout'],'PRIVATE_STDOUT\n')
            successful=subprocess.run([str(directory/'bin/helm'),'version'],capture_output=True)
            self.assertEqual(successful.returncode,0);self.assertEqual(successful.stdout,b'OK\n');self.assertEqual(successful.stderr,b'WARNING\n');self.assertEqual(list((directory/'output').iterdir()),encrypted)
if __name__=='__main__':unittest.main()

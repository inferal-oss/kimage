#!/usr/bin/env python3
"""Optional CI-only Helm failure capture encrypted to a maintainer certificate.

The recipient private key never enters CI. No raw failed-command output is emitted
or written to disk. Normal release/install workflows do not enable this wrapper.
"""
import argparse,json,os,re,shlex,shutil,subprocess,sys,uuid
from pathlib import Path


def runtime_state(args):
    # Only disposable CI fixtures. Everything returned stays inside ciphertext.
    if os.environ.get('GITHUB_ACTIONS')!='true' or '--namespace' not in args:return None
    namespace=args[args.index('--namespace')+1]
    if namespace not in ('upgrade-system','kimage-e2e-system'):return None
    command=['k3s','kubectl'] if shutil.which('k3s') else ['kubectl']
    result={}
    def capture(key,arguments):
        try:
            value=subprocess.run(command+arguments,capture_output=True,timeout=30)
            result[key]=dict(returncode=value.returncode,stdout=value.stdout[:1048576].decode(errors='replace'),stderr=value.stderr[:8192].decode(errors='replace'))
            return value
        except (OSError,subprocess.TimeoutExpired):result[key]=dict(unavailable=True)
    pods=capture('pods',['get','pods','-n',namespace,'-o','json'])
    capture('events',['get','events','-n',namespace,'-o','json'])
    capture('nodes',['get','nodes','-o','json'])
    if pods and pods.returncode==0:
        try:items=json.loads(pods.stdout)['items']
        except (KeyError,ValueError):items=[]
        for pod in items[:12]:
            name=pod.get('metadata',{}).get('name','')
            if not re.fullmatch(r'[a-z0-9][a-z0-9-]{0,252}',name):continue
            for container in pod.get('spec',{}).get('containers',[])[:3]:
                cname=container.get('name','')
                if re.fullmatch(r'[a-z0-9][a-z0-9-]{0,62}',cname):capture('logs/'+name+'/'+cname,['logs',name,'-n',namespace,'-c',cname,'--tail=40','--limit-bytes=16384'])
    return result


def invoke(helm,certificate,output,args):
    os.umask(0o077)
    result=subprocess.run([helm,*args],capture_output=True)
    if result.returncode==0:
        sys.stdout.buffer.write(result.stdout);sys.stderr.buffer.write(result.stderr)
        return 0
    action=args[0] if args and args[0] in ('install','upgrade','rollback','get','history','uninstall','template','version') else 'command'
    name='helm-'+action+'-'+uuid.uuid4().hex
    payload=json.dumps(dict(schema=1,action=action,returncode=result.returncode,stdout=result.stdout.decode(errors='replace'),stderr=result.stderr.decode(errors='replace'),runtime=runtime_state(args) if action in ('install','upgrade') else None)).encode()
    if len(payload)>8*1024*1024:
        sys.stderr.write('PrivateHelmDiagnosticTooLarge\n');return result.returncode
    output=Path(output);output.mkdir(parents=True,exist_ok=True)
    encrypted=output/(name+'.p7m')
    captured=subprocess.run(['openssl','cms','-encrypt','-binary','-aes-256-cbc','-outform','DER','-out',str(encrypted),certificate],input=payload,capture_output=True,timeout=30)
    if captured.returncode:
        encrypted.unlink(missing_ok=True)
        sys.stderr.write('PrivateHelmDiagnosticEncryptionFailed\n')
    else:
        reason='InstallationScopeOverlap' if b'InstallationScopeOverlap' in result.stderr else 'HelmTestCommandFailed'
        sys.stderr.write(reason+': '+action+'; encrypted diagnostic '+encrypted.name+'\n')
    return result.returncode


def main():
    if len(sys.argv)>1 and sys.argv[1]=='--invoke':
        raise SystemExit(invoke(*sys.argv[2:5],sys.argv[5:]))
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--setup',type=Path,required=True);parser.add_argument('--helm',type=Path,required=True)
    args=parser.parse_args();os.umask(0o077)
    certificate=os.environ.get('KIMAGE_HELM_DIAGNOSTIC_CERT','')
    if not 1<=len(certificate)<=16384:raise RuntimeError('InvalidDiagnosticCertificate')
    validated=subprocess.run(['openssl','x509','-noout'],input=certificate.encode(),capture_output=True)
    if validated.returncode:raise RuntimeError('InvalidDiagnosticCertificate')
    root=args.setup.resolve();root.mkdir(parents=True,exist_ok=False)
    cert=root/'recipient.pem';cert.write_text(certificate);cert.chmod(0o600)
    executable=root/'bin/helm';executable.parent.mkdir()
    command=[sys.executable,str(Path(__file__).resolve()),'--invoke',str(args.helm.resolve()),str(cert),str(root/'output')]
    executable.write_text('#!/bin/sh\nexec '+shlex.join(command)+' "$@"\n');executable.chmod(0o700)
    print('Enabled encrypted Helm failure diagnostics; recipient private key stays outside CI.')


if __name__=='__main__':main()

#!/usr/bin/env python3
"""Complete binary upgrade/rollback and cache-absent retained-bundle recovery.

This harness targets only the existing task-owned disconnected two-node fixture.
Logs and recovery checkpoints stay private in --output; evidence contains no keys.
"""
import argparse
import hashlib
import json
import os
import pathlib
import subprocess

p = argparse.ArgumentParser(description=__doc__)
for key in ['kubeconfig','options','initial-bundle','previous-bundle','bundle','installer','probe','admission-credentials','builder-credentials','output']:
    p.add_argument('--' + key, type=pathlib.Path, required=True)
p.add_argument('--case', required=True)
p.add_argument('--linux-roll-forward',action='store_true',help='exercise the packaged native Linux installer as an unprivileged external API client')
a = p.parse_args()
assert a.case.startswith('kimage-binary-') and a.case.replace('-', '').isalnum()
assert 'server: https://127.0.0.1:27643' in a.kubeconfig.read_text()
o = json.loads(a.options.read_text())
assert o['installation'] == 'kimage-clean' and o['namespace'] == 'kimage-system'
a.output.mkdir(parents=True, exist_ok=True)
env = dict(os.environ, KUBECONFIG=str(a.kubeconfig.resolve()))


def trusted(path):
    return 'sha256:' + hashlib.sha256((path / 'inventory.json').read_bytes()).hexdigest()


request = dict(case=a.case,initialInventory=trusted(a.initial_bundle),previousInventory=trusted(a.previous_bundle),targetInventory=trusted(a.bundle),linuxRollForward=a.linux_roll_forward)
request_path = a.output/'request.json'
if request_path.exists():
    assert json.loads(request_path.read_text()) == request,'choose a fresh output for a different binary lifecycle request'
else:
    request_path.write_text(json.dumps(request,indent=2)+'\n')
    request_path.chmod(0o600)


def cluster_args():
    return [part for key,value in o.items() for part in ['--' + ''.join('-' + c.lower() if c.isupper() else c for c in key), str(value)]]


def run(name, args):
    done = a.output / (name + '.done')
    if done.exists():
        return
    log = a.output / (name + '.log')
    with log.open('wb') as stream:
        result = subprocess.run(args, env=env, stdout=stream, stderr=subprocess.STDOUT, timeout=2400)
    log.chmod(0o600)
    assert result.returncode == 0, name + ' failed; inspect its private phase log'
    done.write_text('passed\n')
    print('binary lifecycle phase', name, 'passed', flush=True)


def upgrade(previous, target):
    return [str(a.installer.resolve()), 'upgrade-serving'] + cluster_args() + ['--bundle',str(target.resolve()),'--trusted-inventory',trusted(target),'--previous-bundle',str(previous.resolve()),'--previous-trusted-inventory',trusted(previous)]


for role,bundle in [('previous',a.previous_bundle),('target',a.bundle)]:
    run('check-' + role,[str(a.installer.resolve()),'bundle-check','--bundle',str(bundle.resolve()),'--trusted-inventory',trusted(bundle)])
run('establish-previous', upgrade(a.initial_bundle, a.previous_bundle))
run('before-build', ['python3','scripts/live-direct-build.py','--kubeconfig',str(a.kubeconfig),'--probe',str(a.probe),'--case','kimage-direct-' + a.case + '-before','--node','kimage-clean-2','--output',str(a.output / 'before-build')])
run('upgrade-and-rollback',['python3','scripts/live-serving-upgrade.py','--kubeconfig',str(a.kubeconfig),'--options',str(a.options),'--previous-bundle',str(a.previous_bundle),'--bundle',str(a.bundle),'--installer',str(a.installer),'--output',str(a.output / 'transition'),'--probe',str(a.probe),'--case',a.case])
run('retained-bundle-recovery',['python3','scripts/live-maintenance-recover.py','--kubeconfig',str(a.kubeconfig),'--options',str(a.options),'--bundle',str(a.previous_bundle),'--installer',str(a.installer),'--admission-credentials',str(a.admission_credentials),'--builder-credentials',str(a.builder_credentials),'--output',str(a.output / 'recovery'),'--evict-system-images','--join-node','kimage-clean-2'])
transition = json.loads((a.output / 'transition/evidence.json').read_text())
publication = transition['newVersionPublication']
root_path = a.output / 'new-version.root.json'
root_path.write_text(json.dumps(publication['spec']['root']))
verify = [str(a.installer.resolve()),'verify-remote','--root',str(root_path),'--namespace','kimage-system','--pod','kimage-clean-registry','--origin','https://kimage-clean-registry.kimage-system.svc:5443/','--repository','kimage-app/' + publication['spec']['imageUid'],'--ca',str(pathlib.Path(o['credentials'])/'ca.crt'),'--password-file',str(pathlib.Path(o['credentials'])/'publisher-password')]
run('verify-new-writes-after-old-bundle-recovery',verify)
roll_forward = upgrade(a.previous_bundle,a.bundle)
if a.linux_roll_forward:
    root = pathlib.Path.cwd().resolve()
    def container_path(path):
        return '/work/' + str(pathlib.Path(path).resolve().relative_to(root))
    arguments = roll_forward[1:]
    for index in range(1,len(arguments)):
        if arguments[index-1] in ['--credentials','--bundle','--previous-bundle']:
            arguments[index] = container_path(arguments[index])
    roll_forward = ['docker','run','--rm','--pull=never','--name','kimage-linux-binary-roll-forward','--label','kimage.fixture=kimage-clean','--platform','linux/amd64','--network','host','--read-only','--user',str(os.getuid())+':'+str(os.getgid()),'--cap-drop','ALL','--security-opt','no-new-privileges','--memory','512m','--cpus','1','--tmpfs','/tmp:rw,nosuid,nodev,size=512m,mode=1777','--mount','type=bind,src='+str(root)+',dst=/work,readonly','--workdir','/work','-e','KUBECONFIG='+container_path(a.kubeconfig),'--entrypoint',container_path(a.bundle/'kimage-install-linux-amd64'),'b36c246742b4'] + arguments
run('roll-forward-to-current',roll_forward)
run('verify-new-writes-after-roll-forward',verify)
evidence = dict(transition=transition, recovery=json.loads((a.output/'recovery/evidence.json').read_text()), newerWritesIndependentlyVerifiedAfterOlderBundleRecovery=True, newerWritesIndependentlyVerifiedAfterRollForward=True, finalInventory=trusted(a.bundle),linuxNativeRollForward=a.linux_roll_forward)
(a.output/'evidence.json').write_text(json.dumps(evidence,indent=2)+'\n')
print('real binary upgrade/rollback, cold previous-bundle recovery and final roll-forward passed',flush=True)

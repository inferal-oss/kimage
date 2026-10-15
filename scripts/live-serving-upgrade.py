#!/usr/bin/env python3
"""Real serving binary rollout, interruption/resume, write/read compatibility and rollback."""
import argparse
import datetime
import hashlib
import json
import os
import pathlib
import subprocess
import time

p = argparse.ArgumentParser(description=__doc__)
for key in ['kubeconfig', 'options', 'previous-bundle', 'bundle', 'installer', 'output']:
    p.add_argument('--' + key, type=pathlib.Path, required=True)
p.add_argument('--probe', type=pathlib.Path, required=True)
p.add_argument('--case', required=True)
a = p.parse_args()
assert a.case.startswith('kimage-binary-') and a.case.replace('-', '').isalnum()
assert 'server: https://127.0.0.1:27643' in a.kubeconfig.read_text()
options = json.loads(a.options.read_text())
assert options['installation'] == 'kimage-clean'
a.output.mkdir(parents=True, exist_ok=True)
b = ['kubectl', '--kubeconfig', str(a.kubeconfig), '--request-timeout=45s']
env = dict(os.environ, KUBECONFIG=str(a.kubeconfig.resolve()))


def call(args, body=None, ns='kimage-system'):
    value = subprocess.run(b + ['-n', ns] + args, input=None if body is None else json.dumps(body).encode(), capture_output=True, timeout=100)
    if value.returncode:
        raise RuntimeError(value.stderr.decode()[:1800])
    return value.stdout


def get(kind, name, ns='kimage-system'):
    return json.loads(call(['get', kind, name, '-o', 'json'], ns=ns))


def wait(predicate, label, seconds=600):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        value = predicate()
        if value:
            return value
        time.sleep(1)
    raise RuntimeError(label + ' deadline')


def inventory(path):
    data = (path / 'inventory.json').read_bytes()
    return 'sha256:' + hashlib.sha256(data).hexdigest(), json.loads(data)


old_components = inventory(a.previous_bundle)[1]['components']
new_components = inventory(a.bundle)[1]['components']
assert old_components['registry']['version'] == '3.1.1'
assert new_components['registry']['version'] == '3.1.2'
for role in ['registry', 'gateway', 'proxy']:
    assert old_components[role]['version'] != new_components[role]['version']
    assert old_components[role]['executable']['digest'] != new_components[role]['executable']['digest']
    assert old_components[role]['root'] != new_components[role]['root']


def command(previous, target):
    args = [str(a.installer.resolve()), 'upgrade-serving']
    for key, value in options.items():
        args += ['--' + ''.join('-' + c.lower() if c.isupper() else c for c in key), str(value)]
    args += ['--bundle', str(target.resolve()), '--trusted-inventory', inventory(target)[0], '--previous-bundle', str(previous.resolve()), '--previous-trusted-inventory', inventory(previous)[0]]
    return args


def objects(kind, ns):
    values = json.loads(call(['get', kind, '-o', 'json'], ns=ns))['items']
    return {v['metadata']['name']: dict(uid=v['metadata']['uid'], spec=v['spec']) for v in values}


def serving_pods():
    values = json.loads(call(['get', 'pods', '-l', 'kimage.dev/installation=kimage-clean', '-o', 'json']))['items']
    return {v['metadata']['name']: v for v in values if v['metadata']['name'] in ['kimage-clean-registry', 'kimage-clean-proxy'] or (v['metadata']['name'].startswith('kimage-clean-n-') and v['metadata']['name'].endswith('-proxy'))}


progress = a.output / 'progress.json'
if progress.exists():
    result = json.loads(progress.read_text())
else:
    result = dict(previousInventory=inventory(a.previous_bundle)[0], targetInventory=inventory(a.bundle)[0], pvc=objects('pvc', 'kimage-system'), releases=objects('imagereleases', 'kimage-app'), schemas={k:v for k,v in objects('crd', 'kimage-system').items() if k in ['images.kimage.dev', 'imagereleases.kimage.dev']}, originalPods={k:v['metadata']['uid'] for k,v in serving_pods().items()})
    for fixture, roles in [('qemu-routed', ['registry','gateway','proxy']), ('qemu-second',['proxy'])]:
        cached = json.loads(subprocess.check_output(['python3', 'scripts/isolated-fixture.py', 'request', '--directory', '.local/' + fixture, '--operation', 'images'], timeout=130))
        for role in roles:
            assert not any(new_components[role]['root']['digest'] in ref for item in cached['images'] for ref in item.get('repoDigests', [])), 'replacement must be uncached before warmup'
    result['replacementImagesAbsentBeforeWarmupOnBothNodes'] = True
    progress.write_text(json.dumps(result, indent=2) + '\n')
assert result['previousInventory'] == inventory(a.previous_bundle)[0] and result['targetInventory'] == inventory(a.bundle)[0]


def save():
    progress.write_text(json.dumps(result, indent=2) + '\n')


if not result.get('interruptedAtWriterGap'):
    registry = get('pod', 'kimage-clean-registry')
    assert not registry['metadata'].get('finalizers')
    fault = 'fixture.kimage.dev/interrupt-upgrade'
    call(['patch', 'pod', 'kimage-clean-registry', '--type=merge', '-p', json.dumps(dict(metadata=dict(uid=registry['metadata']['uid'], resourceVersion=registry['metadata']['resourceVersion'], finalizers=[fault])))])
    log = (a.output / 'interrupted.log').open('wb')
    process = subprocess.Popen(command(a.previous_bundle, a.bundle), env=env, stdout=log, stderr=subprocess.STDOUT)
    try:
        def deleting():
            if process.poll() is not None:
                raise RuntimeError('upgrade exited before interruption; inspect private interrupted.log')
            value = get('pod', 'kimage-clean-registry')
            return value if value['metadata'].get('deletionTimestamp') else None
        old = wait(deleting, 'writer transition', 900)
        reader = get('pod', 'kimage-clean-readthrough')
        assert all(v.get('persistentVolumeClaim', {}).get('readOnly') is True for v in reader['spec']['volumes'] if v['name'] == 'data')
        assert get('service', 'kimage-clean-registry')['spec']['selector']['kimage.dev/role'] == 'readthrough'
        target_images = {'127.0.0.1:17641/kimage-system/system-' + role + '@' + new_components[role]['root']['digest'] for role in ['registry','gateway','proxy']}
        warm = [v for v in json.loads(call(['get', 'pods', '-o', 'json']))['items'] if v['metadata']['name'].startswith('kimage-clean-uw-') and v['spec']['containers'][0]['image'] in target_images]
        expected_warmups = {('kimage-clean',role) for role in ['registry','gateway','proxy']} | {('kimage-clean-2','proxy')}
        assert len(warm) == 4 and all(v['status']['phase'] == 'Succeeded' for v in warm)
        assert {(v['spec']['nodeName'],v['spec']['containers'][0]['image'].split('/system-',1)[1].split('@',1)[0]) for v in warm} == expected_warmups
        for node in ['kimage-clean','kimage-clean-2']:
            proxy = next(name for name,pod in serving_pods().items() if pod['spec']['nodeName'] == node and any(c['name'] == 'proxy' for c in pod['spec']['containers']))
            logs = call(['logs',proxy,'--tail=3000']).decode()
            assert all(new_components[role]['root']['digest'] in logs for n,role in expected_warmups if n == node)
        result['replacementRootRequestsObservedOnBothOldProxies'] = True
        process.kill()
        process.wait(timeout=15)
        result['interruptedAtWriterGap'] = True
        result['readOnlyReadthroughObserved'] = True
        result['nodeTargetedWarmupPods'] = [dict(node=v['spec']['nodeName'], image=v['spec']['containers'][0]['image'], imagePullPolicy=v['spec']['containers'][0]['imagePullPolicy']) for v in warm]
        save()
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=15)
        log.close()
        current = get('pod', 'kimage-clean-registry')
        assert current['metadata']['uid'] == registry['metadata']['uid'] and current['metadata'].get('finalizers') == [fault]
        call(['patch', 'pod', 'kimage-clean-registry', '--type=merge', '-p', json.dumps(dict(metadata=dict(uid=current['metadata']['uid'], resourceVersion=current['metadata']['resourceVersion'], finalizers=[])))])
    wait(lambda: not any(p['metadata']['name'] == 'kimage-clean-registry' for p in json.loads(call(['get', 'pods', '-o', 'json']))['items']), 'writer absent', 120)

if not result.get('resumedUpgrade'):
    if not result.get('retainedNormalPullDuringWriterGap'):
        image = get('image', 'kimage-direct-' + a.case + '-before', 'kimage-app')
        release = get('imagerelease', image['status']['releaseRef'], 'kimage-app')
        root = release['spec']['root']['digest']
        cached = json.loads(subprocess.check_output(['python3', 'scripts/isolated-fixture.py', 'request', '--directory', '.local/qemu-routed', '--operation', 'images'], timeout=130))
        assert not any(root in ref for item in cached['images'] for ref in item.get('repoDigests', []))
        pull = a.case + '-writer-gap'
        call(['create', '-f', '-'], dict(apiVersion='v1', kind='Pod', metadata=dict(name=pull), spec=dict(nodeSelector={'kubernetes.io/hostname': 'kimage-clean'}, automountServiceAccountToken=False, containers=[dict(name='app', image='platform://kimage-app/kimage-direct-' + a.case + '-before:1.0.0', imagePullPolicy='IfNotPresent')])), ns='kimage-app')
        def normal_pull():
            pod = get('pod', pull, 'kimage-app')
            return pod if any(c['type'] == 'Ready' and c['status'] == 'True' for c in pod.get('status', {}).get('conditions', [])) else None
        pod = wait(normal_pull, 'uncached retained pull through read-only handover', 240)
        assert pod['spec']['containers'][0]['image'].endswith('@' + root)
        result['retainedNormalPullDuringWriterGap'] = True
        save()
    def lease_available():
        spec = get('lease', 'kimage-clean-serving-upgrade')['spec']
        if not spec.get('holderIdentity'):
            return True
        renewed = datetime.datetime.fromisoformat(spec['renewTime'].replace('Z', '+00:00'))
        return (datetime.datetime.now(datetime.timezone.utc) - renewed).total_seconds() > spec['leaseDurationSeconds'] + 2
    wait(lease_available, 'interrupted owner lease expired', 60)
    with (a.output / 'resume.log').open('wb') as log:
        value = subprocess.run(command(a.previous_bundle, a.bundle), env=env, stdout=log, stderr=subprocess.STDOUT, timeout=900)
    assert value.returncode == 0, 'resume failed; inspect private resume.log'
    expected = inventory(a.bundle)[1]['components']
    for name, pod in serving_pods().items():
        assert pod['metadata']['uid'] != result['originalPods'][name]
        for container in pod['spec']['containers']:
            assert container['image'].endswith('@' + expected[container['name']]['root']['digest'])
    result['resumedUpgrade'] = True
    save()

if not result.get('newVersionPublication'):
    # This publication happens after the new registry binary takes ownership of
    # the retained filesystem. Rollback must read these new writes, not just
    # content that predates the upgrade.
    subprocess.run(['python3', 'scripts/live-direct-build.py', '--kubeconfig', str(a.kubeconfig), '--probe', str(a.probe), '--case', 'kimage-direct-' + a.case + '-after', '--output', str(a.output / 'new-version-build')], check=True, timeout=1000)
    image = get('image', 'kimage-direct-' + a.case + '-after', 'kimage-app')
    release = get('imagerelease', image['status']['releaseRef'], 'kimage-app')
    result['newVersionPublication'] = dict(name=image['metadata']['name'], releaseName=release['metadata']['name'], releaseUid=release['metadata']['uid'], spec=release['spec'])
    result['newVersionExecutables'] = {role:call(['exec', 'kimage-clean-registry' if role in ['registry','gateway'] else 'kimage-clean-proxy', '-c', role, '--', '/registry' if role == 'registry' else '/kimage-' + role, '--version']).decode().strip() for role in ['registry','gateway','proxy']}
    assert '3.1.2' in result['newVersionExecutables']['registry']
    assert '0.2.0' in result['newVersionExecutables']['proxy']
    assert '0.2.0' in result['newVersionExecutables']['gateway']
    save()

if not result.get('compatibleRollback'):
    with (a.output / 'rollback.log').open('wb') as log:
        value = subprocess.run(command(a.bundle, a.previous_bundle), env=env, stdout=log, stderr=subprocess.STDOUT, timeout=900)
    assert value.returncode == 0, 'rollback failed; inspect private rollback.log'
    expected = inventory(a.previous_bundle)[1]['components']
    for pod in serving_pods().values():
        for container in pod['spec']['containers']:
            assert container['image'].endswith('@' + expected[container['name']]['root']['digest'])
    result['compatibleRollback'] = True
    save()

if not result.get('newVersionWritesReadableAfterRollback'):
    publication = result['newVersionPublication']
    release = get('imagerelease', publication['releaseName'], 'kimage-app')
    assert release['metadata']['uid'] == publication['releaseUid'] and release['spec'] == publication['spec']
    registry_version = call(['exec', 'kimage-clean-registry', '-c', 'registry', '--', '/registry', '--version']).decode().strip()
    assert '3.1.1' in registry_version
    result['rollbackRegistryExecutableVersion'] = registry_version
    # Pick the node on which the new release has never run, proving the old
    # binaries serve persisted bytes from the new binary through normal pulls.
    built = get('pod', publication['name'] + '-workload', 'kimage-app')
    node = 'kimage-clean-2' if built['spec']['nodeName'] == 'kimage-clean' else 'kimage-clean'
    directory = '.local/qemu-second' if node == 'kimage-clean-2' else '.local/qemu-routed'
    cached = json.loads(subprocess.check_output(['python3', 'scripts/isolated-fixture.py', 'request', '--directory', directory, '--operation', 'images'], timeout=130))
    root = release['spec']['root']['digest']
    assert not any(root in ref for item in cached['images'] for ref in item.get('repoDigests', []))
    pull = a.case + '-rollback-pull'
    call(['create', '-f', '-'], dict(apiVersion='v1', kind='Pod', metadata=dict(name=pull), spec=dict(nodeSelector={'kubernetes.io/hostname':node}, automountServiceAccountToken=False, containers=[dict(name='app', image='platform://kimage-app/' + publication['name'] + ':1.0.0', imagePullPolicy='IfNotPresent')])), ns='kimage-app')
    def rollback_pull():
        pod = get('pod', pull, 'kimage-app')
        return pod if any(c['type'] == 'Ready' and c['status'] == 'True' for c in pod.get('status', {}).get('conditions', [])) else None
    pod = wait(rollback_pull, 'new-version writes after binary rollback', 240)
    assert pod['spec']['containers'][0]['image'].endswith('@' + root)
    result['newVersionWritesReadableAfterRollback'] = dict(root=root, node=node, workloadUid=pod['metadata']['uid'], absentFromTargetCache=True, normalIfNotPresentPull=True)
    save()

assert objects('pvc', 'kimage-system') == result['pvc']
current_releases = objects('imagereleases', 'kimage-app')
assert all(current_releases.get(k) == v for k,v in result['releases'].items())
assert {k:v for k,v in objects('crd', 'kimage-system').items() if k in result['schemas']} == result['schemas']
result.update(retainedPvcAndReleaseUidsSpecsPreserved=True, schemaUidsSpecsPreserved=True, executableTransitions={role:dict(previousVersion=old_components[role]['version'], targetVersion=new_components[role]['version'], previousExecutable=old_components[role]['executable']['digest'], targetExecutable=new_components[role]['executable']['digest']) for role in ['registry','gateway','proxy']}, transitionScope='real Distribution 3.1.1 -> 3.1.2 and serving 0.1.0 -> 0.2.0 binaries, reverse rollback; filesystem backend without migration')
save()
(a.output / 'evidence.json').write_text(json.dumps({k:v for k,v in result.items() if k not in ['pvc','releases','schemas']}, indent=2) + '\n')
print('native two-node serving binary upgrade, interrupted resume, new writes and compatible rollback passed', flush=True)

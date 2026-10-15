#!/usr/bin/env python3
"""Node identity replacement, CRI cache eviction and ordinary API restart tests.

The provider owns only the two isolated VMs; product recovery remains API-only.
"""
import argparse
import copy
import hashlib
import json
import os
import pathlib
import shutil
import subprocess
import time

p = argparse.ArgumentParser(description=__doc__)
for key in ['kubeconfig', 'options', 'bundle', 'installer', 'delivery', 'output']:
    p.add_argument('--' + key, type=pathlib.Path, required=True)
a = p.parse_args()
assert 'server: https://127.0.0.1:27643' in a.kubeconfig.read_text()
options = json.loads(a.options.read_text())
assert options['installation'] == 'kimage-clean'
a.output.mkdir(parents=True, exist_ok=True)
b = ['kubectl', '--kubeconfig', str(a.kubeconfig), '--request-timeout=45s']
env = dict(os.environ, KUBECONFIG=str(a.kubeconfig.resolve()))
trusted = 'sha256:' + hashlib.sha256((a.bundle / 'inventory.json').read_bytes()).hexdigest()
delivery = json.loads(a.delivery.read_text())
root = delivery['root']['digest']
case = 'kimage-two-a2'


def call(args, body=None, ns='kimage-app'):
    result = subprocess.run(b + ['-n', ns] + args, input=None if body is None else json.dumps(body).encode(), capture_output=True, timeout=100)
    if result.returncode:
        raise RuntimeError(result.stderr.decode()[:1800])
    return result.stdout


def get(kind, name, ns='kimage-app'):
    return json.loads(call(['get', kind, name, '-o', 'json'], ns=ns))


def wait(predicate, label, seconds=300):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        value = predicate()
        if value:
            return value
        time.sleep(2)
    raise RuntimeError(label + ' deadline')


def ready(obj):
    return not obj['metadata'].get('deletionTimestamp') and any(c['type'] == 'Ready' and c['status'] == 'True' for c in obj.get('status', {}).get('conditions', []))


def replicas(count):
    call(['scale', 'deployment', case, '--replicas=' + str(count)])


def workloads():
    return json.loads(call(['get', 'pods', '-l', 'kimage.dev/delivery-case=' + case, '-o', 'json']))['items']


def running():
    return next((pod for pod in workloads() if ready(pod)), None)


def provider(operation, images=None):
    args = ['python3', 'scripts/isolated-fixture.py', 'request', '--directory', '.local/qemu-second', '--operation', operation]
    if images:
        path = a.output / 'evict-images.json'
        path.write_text(json.dumps(images))
        args += ['--images', str(path)]
    return json.loads(subprocess.check_output(args, timeout=130))


def images_have(reference):
    value = provider('images')
    return any(reference in item.get('repoDigests', []) for item in value['images'])


def native_join():
    args = [str(a.installer.resolve()), 'join-node']
    for key, value in options.items():
        args += ['--' + ''.join('-' + c.lower() if c.isupper() else c for c in key), str(value)]
    args += ['--bundle', str(a.bundle.resolve()), '--trusted-inventory', trusted, '--target-node', 'kimage-clean-2', '--recover']
    subprocess.run(args, env=env, check=True, timeout=600)


def restart(name):
    old = get('pod', name, 'kimage-system')
    assert old['metadata']['labels']['kimage.dev/installation'] == 'kimage-clean'
    desired = dict(apiVersion='v1', kind='Pod', metadata={key: old['metadata'][key] for key in ['name', 'labels', 'annotations'] if key in old['metadata']}, spec=old['spec'])
    call(['delete', 'pod', name, '--wait=true', '--timeout=90s'], ns='kimage-system')
    call(['create', '-f', '-'], desired, ns='kimage-system')
    new = wait(lambda: (lambda v: v if ready(v) else None)(get('pod', name, 'kimage-system')), name + ' restart')
    assert new['metadata']['uid'] != old['metadata']['uid']
    return dict(oldPodUid=old['metadata']['uid'], podUid=new['metadata']['uid'])


# Save durable receipts before destructive fixture faults, so a failed test can
# resume without asserting that its own already-performed replacement is fresh.
receipt_path = a.output / 'progress.json'
if receipt_path.exists():
    result = json.loads(receipt_path.read_text())
else:
    release = next(item for item in json.loads(call(['get', 'imagereleases', '-o', 'json']))['items'] if item['metadata']['uid'] == delivery['releaseUid'])
    node = get('node', 'kimage-clean-2')
    result = dict(releaseName=release['metadata']['name'], releaseUid=release['metadata']['uid'], releaseSpec=release['spec'], oldNodeUid=node['metadata']['uid'])
    receipt_path.write_text(json.dumps(result, indent=2) + '\n')


def save():
    receipt_path.write_text(json.dumps(result, indent=2) + '\n')


if 'newNodeUid' not in result:
    replicas(0)
    wait(lambda: not workloads(), 'workload stopped')
    subprocess.run(['python3', 'scripts/isolated-fixture.py', 'stop', '--directory', '.local/qemu-second'], check=True, timeout=30)
    shutil.copyfile('scripts/fixtures/qemu/agent.py', '.local/qemu-second/prerequisites/fixture-agent.py')
    current = get('node', 'kimage-clean-2')
    assert current['metadata']['uid'] == result['oldNodeUid']
    call(['delete', 'node', 'kimage-clean-2', '--wait=true', '--timeout=60s'])
    subprocess.run(['python3', 'scripts/isolated-fixture.py', 'start', '--directory', '.local/qemu-second'], check=True, timeout=30)
    def new_node():
        value = json.loads(call(['get', 'nodes', '-o', 'json']))
        return next((n for n in value['items'] if n['metadata']['name'] == 'kimage-clean-2' and n['metadata']['uid'] != result['oldNodeUid'] and ready(n)), None)
    node = wait(new_node, 'replacement node registered', 360)
    dry = dict(apiVersion='v1', kind='Pod', metadata=dict(name='kimage-replaced-node-dry-run'), spec=dict(containers=[dict(name='app', image='platform://kimage-app/' + case + ':1.0.0')]))
    mutated = json.loads(call(['create', '--dry-run=server', '-f', '-', '-o', 'json'], dry))
    terms = mutated['spec']['affinity']['nodeAffinity']['requiredDuringSchedulingIgnoredDuringExecution']['nodeSelectorTerms']
    assert all(field['values'] == ['kimage-clean'] for term in terms for field in term['matchFields'] if field['key'] == 'metadata.name')
    result.update(newNodeUid=node['metadata']['uid'], staleNodeUidExcluded=True)
    save()

if not result.get('nativeRecoveryIdempotent'):
    native_join()
    name = 'kimage-clean-n-' + hashlib.sha256(result['newNodeUid'].encode()).hexdigest()[:16] + '-proxy'
    proxy = get('pod', name, 'kimage-system')
    assert ready(proxy) and proxy['metadata']['annotations']['kimage.dev/node-uid'] == result['newNodeUid']
    native_join()
    assert get('pod', name, 'kimage-system')['metadata']['uid'] == proxy['metadata']['uid']
    result.update(proxyName=name, recoveredProxyUid=proxy['metadata']['uid'], nativeRecoveryIdempotent=True)
    save()

if not result.get('cacheEvictionNormalRepull'):
    replicas(1)
    pod = wait(running, 'replacement-node workload')
    reference = pod['spec']['containers'][0]['image']
    assert pod['spec']['nodeName'] == 'kimage-clean-2' and reference.endswith('@' + root)
    assert images_have(reference)
    replicas(0)
    wait(lambda: not workloads(), 'cache eviction workload stopped')
    evicted = provider('evict_retained_images', [reference])
    assert evicted['code'] == 0 and not images_have(reference)
    replicas(1)
    new = wait(running, 'normal repull after eviction')
    assert new['metadata']['uid'] != pod['metadata']['uid'] and new['spec']['containers'][0]['image'] == reference
    assert root.encode() in call(['logs', result['proxyName'], '--tail=3000'], ns='kimage-system')
    result.update(cacheEvictionNormalRepull=True, repullPodUid=new['metadata']['uid'], reference=reference)
    save()

if 'proxyRestart' not in result:
    pod = wait(running, 'running workload')
    container = pod['status']['containerStatuses'][0]['containerID']
    result['proxyRestart'] = restart(result['proxyName'])
    after = wait(running, 'workload after proxy restart')
    assert after['metadata']['uid'] == pod['metadata']['uid'] and after['status']['containerStatuses'][0]['containerID'] == container
    save()

if 'registryRestart' not in result:
    pod = wait(running, 'running workload')
    container = pod['status']['containerStatuses'][0]['containerID']
    result['registryRestart'] = restart('kimage-clean-registry')
    after = wait(running, 'workload after registry restart')
    assert after['metadata']['uid'] == pod['metadata']['uid'] and after['status']['containerStatuses'][0]['containerID'] == container
    result['runningContainerSurvivedRestarts'] = True
    save()

release = get('imagerelease', result['releaseName'])
assert release['metadata']['uid'] == result['releaseUid'] and release['spec'] == result['releaseSpec']
assert not provider('external_probe')['externalReachable']
result['releaseAndExecutionUnchanged'] = True
save()
sanitized = {key: value for key, value in result.items() if key not in ['releaseSpec', 'reference']}
(a.output / 'evidence.json').write_text(json.dumps(sanitized, indent=2) + '\n')
print('node UID replacement, native proxy recovery, cache eviction/normal repull and service restarts passed', flush=True)

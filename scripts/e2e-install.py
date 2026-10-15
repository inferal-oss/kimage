#!/usr/bin/env python3
"""Real API-only installation, native build/pull, policy, and scope acceptance.

Restricted to a disposable GitHub-hosted fixture. Never points at a user's
current context and never imports kimage images into the node runtime.
"""
import argparse
import base64
import copy
import hashlib
import json
import os
from pathlib import Path
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--scope', choices=['namespace', 'cluster'], required=True)
    parser.add_argument('--bundle', type=Path, required=True)
    parser.add_argument('--installer', type=Path, required=True)
    parser.add_argument('--probe', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    assert os.environ.get('GITHUB_ACTIONS') == 'true' and os.environ.get('RUNNER_ENVIRONMENT') == 'github-hosted'
    assert os.environ.get('KIMAGE_E2E_ARCHITECTURE') in ('amd64', 'arm64')
    arch = os.environ['KIMAGE_E2E_ARCHITECTURE']
    kubeconfig = Path(os.environ['KUBECONFIG']).resolve()
    assert kubeconfig.name == 'config' and 'e2e-cluster' in str(kubeconfig)
    args.output.mkdir(parents=True, exist_ok=True)
    command = ['k3s', 'kubectl', '--kubeconfig', str(kubeconfig), '--request-timeout=20s']
    report = dict(scope=args.scope, platform='linux/'+arch, checks=[], complete=False)
    system, app, other, excluded, installation = 'e2e-system', 'e2e-app', 'e2e-other', 'e2e-excluded', 'e2e'

    def run(arguments, value=None, ok=True, timeout=90):
        result = subprocess.run([str(v) for v in arguments], input=None if value is None else json.dumps(value), text=True, capture_output=True, timeout=timeout)
        if ok and result.returncode:
            raise RuntimeError('Command failed: ' + str(arguments[:4]) + '\n' + result.stderr[-3000:])
        return result

    def kubectl(arguments, value=None, ok=True):
        return run(command + arguments, value, ok)

    def get(kind, name=None, ns=app):
        arguments = ['get', kind] + ([name] if name else []) + ['-n', ns, '-o', 'json']
        result = kubectl(arguments, ok=False)
        if result.returncode:
            if 'NotFound' in result.stderr:
                return None
            raise RuntimeError(result.stderr[-2000:])
        return json.loads(result.stdout)

    def create(body, ns=app, dry_run=False, ok=True):
        body = copy.deepcopy(body)
        body.setdefault('metadata', {})['namespace'] = ns
        return kubectl(['create', '-f', '-', '-o', 'json'] + (['--dry-run=server'] if dry_run else []), body, ok)

    def wait(test, label, seconds=300):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            result = test()
            if result:
                return result
            time.sleep(2)
        raise RuntimeError('Timed out: '+label)

    def check(label):
        report['checks'].append(label)
        print('PASS: ' + label, flush=True)
        (args.output / 'results.json').write_text(json.dumps(report, indent=2)+'\n')

    assert kubectl(['config', 'view', '--minify', '-o', 'jsonpath={.clusters[0].cluster.server}']).stdout == 'https://127.0.0.1:6443'
    nodes = json.loads(kubectl(['get', 'nodes', '-o', 'json']).stdout)['items']
    assert len(nodes) == 1 and nodes[0]['metadata']['name'] == 'kimage-e2e'
    assert nodes[0]['status']['nodeInfo']['architecture'] == arch
    report['runtime'] = {field:nodes[0]['status']['nodeInfo'][field] for field in ['architecture','operatingSystem','kernelVersion','osImage','kubeletVersion','containerRuntimeVersion']}
    def cached_prerequisites():
        value = json.loads(kubectl(['get', 'nodes', '-o', 'json']).stdout)['items'][0]
        images = value['status'].get('images', [])
        return images if any('mirrored-pause@sha256:' in name for image in images for name in image.get('names', [])) else None
    images = wait(cached_prerequisites, 'kubelet cached prerequisite image report', 180)
    assert not any(name.startswith('127.0.0.1:') for image in images for name in image.get('names', []))
    carriers = sorted(name for image in images for name in image.get('names', []) if 'mirrored-pause@sha256:' in name)
    assert carriers, 'cached prerequisite carrier required'
    for namespace in [system, app, other, excluded]:
        kubectl(['create', 'namespace', namespace])
    registry, admission, builder = (args.output / name for name in ('registry', 'admission', 'builder'))
    run(['python3', ROOT/'scripts/prepare-test-credentials.py', '--dir', registry, '--registry-dns', 'e2e-registry.e2e-system.svc'])
    run(['python3', ROOT/'scripts/prepare-test-credentials.py', '--dir', admission, '--registry-dns', 'e2e-admission.e2e-system.svc'])
    run(['python3', ROOT/'scripts/prepare-builder-test-credentials.py', '--dir', builder, '--dns', 'e2e-builder.e2e-system.svc'])
    digest = 'sha256:'+hashlib.sha256((args.bundle/'inventory.json').read_bytes()).hexdigest()
    common = ['--scope', args.scope, '--namespace', system, '--installation', installation, '--node', 'kimage-e2e', '--carrier', carriers[0], '--runtime-profile', 'k3s', '--storage-class', 'local-path', '--port', '17701', '--credentials', registry]
    common += ['--application-namespace', app] if args.scope == 'namespace' else ['--exclude-namespace', excluded]
    local = ['--bundle', args.bundle.resolve(), '--trusted-inventory', digest]
    try:
        result = run([args.installer.resolve(), 'install'] + common + local + ['--admission-credentials', admission, '--builder-credentials', builder], timeout=1200, ok=False)
        (args.output/'install.log').write_text(result.stdout+result.stderr)
        assert result.returncode == 0, 'installation failed: '+result.stderr[-3000:]
        check('fresh API-only '+args.scope+' install and authenticated native worker readiness')
        pvc = get('pvc', 'e2e-data', system)
        assert pvc, 'registry PVC missing'
        pvc_uid = pvc['metadata']['uid']
        # Normal workloads must never inherit the publisher's cluster permissions.
        for resource, ns, expected in [('images.kimage.dev', app, True), ('images.kimage.dev', other, args.scope == 'cluster')]:
            result = kubectl(['auth', 'can-i', 'get', resource, '-n', ns, '--as=system:serviceaccount:e2e-system:e2e-operator'], ok=False)
            assert (result.stdout.strip() == 'yes') == expected
        check('operator RBAC matches installation scope')
        probe = base64.b64encode(args.probe.read_bytes()).decode()
        for ns in [app, other]:
            create(dict(apiVersion='v1', kind='ConfigMap', metadata=dict(name='source'), binaryData={'hello-'+arch: probe}), ns)
        def image(name='service'):
            return dict(apiVersion='kimage.dev/v1alpha1', kind='Image', metadata=dict(name=name), spec=dict(version='1.0.0', source=dict(build=dict(format='Containerfile', definition=dict(inline='FROM scratch\nARG TARGETARCH\nCOPY hello-${TARGETARCH} /probe\nENTRYPOINT ["/probe"]\n'), execution=dict(strategy='Direct', builderRef='default'), inputs=dict(contextPath='/workspace/context', volumes=[dict(name='source', configMap=dict(name='source', defaultMode=365))], volumeMounts=[dict(name='source', mountPath='/workspace/context', readOnly=True)])))))
        create(image())
        wait(lambda: any(c.get('reason')=='AwaitingDemand' for c in (get('images.kimage.dev', 'service') or {}).get('status', {}).get('conditions', [])), 'default awaiting demand')
        time.sleep(8)
        assert not get('imagedemands.kimage.dev', ns=system)['items']
        assert len(get('images.kimage.dev')['items']) == 1
        check('automatic Image remains idle without consumers')
        def pod(selector, name='consumer'):
            return dict(apiVersion='v1', kind='Pod', metadata=dict(name=name), spec=dict(nodeSelector={'kubernetes.io/arch': arch}, containers=[dict(name='app', image=selector)], restartPolicy='Never'))
        selector = 'platform://'+app+'/service:^1.0.0'
        dry = create(pod(selector), dry_run=True, ok=False)
        assert dry.returncode and 'ImageBuildPending' in dry.stderr
        assert not get('imagedemands.kimage.dev', ns=system)['items']
        check('dry-run reports pending without recording build demand')
        template = pod(selector)['spec']
        deployment = dict(apiVersion='apps/v1', kind='Deployment', metadata=dict(name='consumer'), spec=dict(replicas=1, selector=dict(matchLabels=dict(app='consumer')), template=dict(metadata=dict(labels=dict(app='consumer')), spec=dict(template, restartPolicy='Always'))))
        create(deployment)
        wait(lambda: get('imagedemands.kimage.dev', ns=system)['items'], 'real controller demand')
        def ready(name='service', ns=app):
            value = get('images.kimage.dev', name, ns)
            return value if any(c['type']=='Ready' and c['status']=='True' for c in value.get('status', {}).get('conditions', [])) else None
        built = wait(ready, 'native demand-triggered build', 600)
        release = get('imagereleases.kimage.dev', built['status']['releaseRef'])
        assert release['status']['verified'] and release['spec']['platform'] == dict(os='linux', architecture=arch)
        assert release['spec']['provenance']['execution']['platform'] == dict(os='linux', architecture=arch)
        demand = get('imagedemands.kimage.dev', ns=system)['items'][0]
        assert demand['spec']['platforms'] == [dict(os='linux', architecture=arch)]
        children = [v for v in get('images.kimage.dev')['items'] if v['metadata'].get('labels', {}).get('kimage.dev/platform-parent') == built['metadata']['uid']]
        assert len(children) == 1 and children[0]['spec']['platform']['architecture'] == arch
        run(command+['rollout', 'status', 'deployment/consumer', '-n', app, '--timeout=300s'], timeout=320)
        running = [v for v in get('pods')['items'] if v['metadata'].get('labels', {}).get('app')=='consumer']
        assert running and all(v['spec']['containers'][0]['image'].endswith('@'+release['spec']['root']['digest']) and v['spec']['nodeName']=='kimage-e2e' for v in running)
        check('only requested native platform built and pulled by Deployment')
        encoded = create(pod('platform://'+app+'/service?version=%5E1.0.0', 'encoded'), dry_run=True)
        assert json.loads(encoded.stdout)['spec']['containers'][0]['image'].endswith('@'+release['spec']['root']['digest'])
        check('readable and percent-encoded selectors resolve the same native digest')
        # Exercise eight built-in/custom controller Pod templates, including init
        # containers and native sidecars, through the real admission server.
        run(['python3', ROOT/'scripts/live-workload-consumers.py', '--kubeconfig', kubeconfig, '--namespace', app, '--image', 'service', '--ci-owned-cluster', '--output', args.output/'controllers.json'], timeout=900)
        check('built-in and custom controllers, init containers and sidecars use native releases')
        foreign = create(pod(selector, 'foreign'), ns=other, dry_run=True, ok=False)
        if args.scope == 'cluster':
            assert foreign.returncode and 'CrossNamespaceDenied' in foreign.stderr
            create(image(), ns=other)
            local_other = 'platform://'+other+'/service:^1.0.0'
            other_deployment = copy.deepcopy(deployment)
            other_deployment['spec']['template']['spec']['containers'][0]['image'] = local_other
            create(other_deployment, ns=other)
            second = wait(lambda: ready(ns=other), 'second namespace native build', 600)
            assert second['metadata']['uid'] != built['metadata']['uid']
            run(command+['rollout', 'status', 'deployment/consumer', '-n', other, '--timeout=300s'], timeout=320)
            assert get('imagereleases.kimage.dev', second['status']['releaseRef'], other)['spec']['imageUid'] == second['metadata']['uid']
            untouched = create(pod('platform://'+excluded+'/missing:1.0.0'), ns=excluded, dry_run=True)
            assert json.loads(untouched.stdout)['spec']['containers'][0]['image'].startswith('platform://')
            check('cluster scope handles independent namespaces, rejects cross-namespace consumption and skips exclusions')
        else:
            assert foreign.returncode == 0 and json.loads(foreign.stdout)['spec']['containers'][0]['image'] == selector
            check('namespace scope leaves other namespaces untouched')
        config = get('configmap', 'e2e-config', system)
        config['data']['configuration.json'] = json.dumps(dict(schema=1, buildPolicy='Eager'))
        kubectl(['replace', '-f', '-'], config)
        create(image('eager'))
        wait(lambda: ready('eager'), 'eager native build without consumer', 600)
        assert all(v['spec']['imageName'] != 'eager' for v in get('imagedemands.kimage.dev', ns=system)['items'])
        check('administrator ConfigMap eager policy builds without workload demand')
        run([args.installer.resolve(), 'uninstall'] + common, timeout=300)
        assert get('pvc', 'e2e-data', system)['metadata']['uid'] == pvc_uid
        assert get('images.kimage.dev', 'service')['metadata']['uid'] == built['metadata']['uid']
        check('uninstall preserves registry storage and published image resources')
        report['complete'] = True
    finally:
        # Keep logs and non-sensitive object status for CI diagnosis. Credential
        # directories, Secrets and raw input ConfigMaps are never uploaded.
        for resource in ['pods', 'events', 'images.kimage.dev', 'imagereleases.kimage.dev']:
            for ns in [system, app, other]:
                result = kubectl(['get', resource, '-n', ns, '-o', 'json'], ok=False)
                (args.output/(ns+'-'+resource+'.json')).write_text(result.stdout)
        (args.output/'results.json').write_text(json.dumps(report, indent=2)+'\n')
    print('E2E complete: '+args.scope+' linux/'+arch)


if __name__ == '__main__':
    main()

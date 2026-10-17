#!/usr/bin/env python3
"""Real registry-pulled installation, native build/pull, policy, and scope acceptance.

Restricted to a disposable GitHub-hosted fixture. Never points at a user's
current context and never imports kimage images into the node runtime.
"""
import argparse
import base64
import copy
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('log_assertions', ROOT/'scripts/e2e-log-assertions.py')
log_assertions = importlib.util.module_from_spec(spec)
spec.loader.exec_module(log_assertions)


def prerequisite_images(node):
    images = node.get('status', {}).get('images') or []
    return images if any('mirrored-pause@sha256:' in name for image in images for name in (image.get('names') or [])) else None


def fixture_cache_ids(images):
    """Only loopback references created by this disposable CI fixture."""
    result=[]
    for image in images:
        names=(image.get('repoTags') or [])+(image.get('repoDigests') or [])
        loopback=[name for name in names if name.startswith('127.0.0.1:')]
        assert all(name.startswith(('127.0.0.1:14901/kimage/','127.0.0.1:17702/upgrade-app/')) for name in loopback), 'Unexpected loopback image outside the owned CI fixture'
        if loopback:
            identity=image['id']
            assert identity.startswith('sha256:') and len(identity)==71 and all(c in '0123456789abcdef' for c in identity[7:])
            result.append(identity)
    return sorted(set(result))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--scope', choices=['namespace', 'cluster'], required=True)
    parser.add_argument('--image-catalog', type=Path, required=True)
    parser.add_argument('--mode', choices=['cli','helm'], default='cli')
    parser.add_argument('--chart', type=Path)
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
    report = dict(mode=args.mode, scope=args.scope, platform='linux/'+arch, checks=[], complete=False)
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
        return prerequisite_images(value)
    # Schema-5 predecessor acceptance must run first. Retire only its loopback
    # fixture images so this stage still proves genuinely cold registry pulls.
    cached=json.loads(run(['sudo','k3s','crictl','images','--output','json']).stdout)['images']
    for identity in fixture_cache_ids(cached):run(['sudo','k3s','crictl','rmi',identity])
    assert not fixture_cache_ids(json.loads(run(['sudo','k3s','crictl','images','--output','json']).stdout)['images'])
    def cold_prerequisites():
        images=cached_prerequisites()
        return images if images and not any(name.startswith('127.0.0.1:') for image in images for name in (image.get('names') or [])) else None
    wait(cold_prerequisites, 'cold fixture cache reported by kubelet', 180)
    check('owned predecessor image cache retired; fresh native registry pulls remain required')
    for namespace in [system, app, other, excluded]:
        kubectl(['create', 'namespace', namespace])
    catalog=json.loads(args.image_catalog.read_bytes())
    assert catalog['repository']=='127.0.0.1:14901/kimage'
    common=['--namespace',system,'--installation',installation,'--scope',args.scope,'--runtime-profile','k3s','--storage-class','local-path']
    common += ['--application-namespace',app] if args.scope=='namespace' else ['--exclude-namespace',excluded,'--exclude-namespace','upgrade-app']
    def install():
        if args.mode=='cli':
            return run([args.installer.resolve(),'install',*common,'--image-catalog',args.image_catalog.resolve()],timeout=1200,ok=False)
        assert args.chart
        values=dict(scope=args.scope,applicationNamespace=app if args.scope=='namespace' else '',excludedNamespaces=[] if args.scope=='namespace' else [excluded,'upgrade-app'],runtimeProfile='k3s',persistence=dict(storageClass='local-path'),images=dict(repository=catalog['repository']))
        path=args.output/'helm-values.json';path.write_text(json.dumps(values))
        return run(['helm','upgrade','--install',installation,args.chart.resolve(),'--namespace',system,'-f',path,'--wait','--wait-for-jobs','--timeout','15m'],timeout=1200,ok=False)
    try:
        result = install()
        (args.output/'install.log').write_text(result.stdout+result.stderr)
        assert result.returncode == 0, 'installation failed: '+result.stderr[-3000:]
        check('fresh '+args.mode+' '+args.scope+' install and authenticated native worker readiness')
        pods=get('pods',ns=system)['items']
        assert pods and all(c['image'].startswith(catalog['repository']+'/') and '@sha256:' in c['image'] for pod in pods for c in pod['spec']['containers'])
        maps=get('configmaps',ns=system)['items']
        assert all(not v.get('binaryData') for v in maps)
        assert not any('launcher' in json.dumps(v) or 'chunk-' in v['metadata']['name'] for v in maps)
        check('components use ordinary digest-pinned registry pulls with no executable ConfigMaps')
        credential_uids={v['metadata']['name']:v['metadata']['uid'] for v in get('secrets',ns=system)['items'] if v['metadata'].get('labels',{}).get('kimage.dev/installation')==installation}
        # A second installation may not intercept an existing scope.
        if args.mode=='cli':
            overlap_common=list(common);overlap_common[overlap_common.index(installation)]='e2e-overlap'
            overlap=run([args.installer.resolve(),'install',*overlap_common,'--image-catalog',args.image_catalog.resolve()],timeout=90,ok=False)
        else:
            overlap=run(['helm','install','e2e-overlap',args.chart.resolve(),'--namespace',system,'-f',args.output/'helm-values.json'],timeout=90,ok=False)
        assert overlap.returncode and 'InstallationScopeOverlap' in overlap.stderr, 'overlapping installation was not refused before mutation'
        assert not any(v['metadata'].get('labels',{}).get('kimage.dev/installation')=='e2e-overlap' for v in get('configmaps',ns=system)['items'])
        check('overlapping installation refused before creating resources')
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
            return dict(apiVersion='kimage.dev/v1alpha1', kind='Image', metadata=dict(name=name), spec=dict(version='1.0.0', source=dict(build=dict(format='Containerfile', definition=dict(inline='FROM scratch\nARG TARGETARCH\nCOPY hello-${TARGETARCH} /probe\nRUN ["/probe", "wait", "0"]\nENTRYPOINT ["/probe"]\n'), execution=dict(strategy='Direct', builderRef='default'), inputs=dict(contextPath='/workspace/context', volumes=[dict(name='source', configMap=dict(name='source', defaultMode=365))], volumeMounts=[dict(name='source', mountPath='/workspace/context', readOnly=True)])))))
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
        (args.output/'dry-run.log').write_text(dry.stderr)
        assert dry.returncode and ('ImageBuildPending' in dry.stderr or 'Server dry-run records no build demand' in dry.stderr), dry.stderr
        assert not get('imagedemands.kimage.dev', ns=system)['items']
        check('dry-run reports pending without recording build demand')
        template = pod(selector)['spec']
        deployment = dict(apiVersion='apps/v1', kind='Deployment', metadata=dict(name='consumer'), spec=dict(replicas=1, selector=dict(matchLabels=dict(app='consumer')), template=dict(metadata=dict(labels=dict(app='consumer')), spec=dict(template, restartPolicy='Always'))))
        create(deployment)
        wait(lambda: get('imagedemands.kimage.dev', ns=system)['items'], 'real controller demand')
        def ready(name='service', ns=app):
            value = get('images.kimage.dev', name, ns)
            if value.get('status', {}).get('observedGeneration') == value['metadata'].get('generation',1) and any(c['type']=='Ready' and c['status']=='True' for c in value.get('status', {}).get('conditions', [])):
                return value
            variants = get('images.kimage.dev', ns=ns)['items']
            for variant in variants:
                if variant['metadata'].get('labels', {}).get('kimage.dev/platform-parent') == value['metadata']['uid'] and (variant.get('status', {}).get('operation') or {}).get('phase') == 'Failed':
                    reasons = [c.get('reason') for c in variant['status'].get('conditions', [])]
                    raise RuntimeError('native variant failed: '+str(reasons))
            return None
        built = wait(ready, 'native demand-triggered build', 600)
        release = get('imagereleases.kimage.dev', built['status']['releaseRef'])
        assert release['status']['verified'] and release['spec']['platform'] == dict(os='linux', architecture=arch)
        assert release['spec']['provenance']['execution']['platform'] == dict(os='linux', architecture=arch)
        assert release['spec']['provenance']['execution']['executedSteps'] > 0
        demand = get('imagedemands.kimage.dev', ns=system)['items'][0]
        assert demand['spec']['platforms'] == [dict(os='linux', architecture=arch)]
        children = [v for v in get('images.kimage.dev')['items'] if v['metadata'].get('labels', {}).get('kimage.dev/platform-parent') == built['metadata']['uid']]
        assert len(children) == 1 and children[0]['spec']['platform']['architecture'] == arch
        run(command+['rollout', 'status', 'deployment/consumer', '-n', app, '--timeout=300s'], timeout=320)
        running = [v for v in get('pods')['items'] if v['metadata'].get('labels', {}).get('app')=='consumer']
        assert running and all(v['spec']['containers'][0]['image'].endswith('@'+release['spec']['root']['digest']) and v['spec']['nodeName']=='kimage-e2e' for v in running)
        check('only requested native platform built and pulled by Deployment')
        for resource in ['pods','jobs']:
            operator='system:serviceaccount:'+system+':'+installation+'-operator'
            allowed=kubectl(['auth','can-i','patch',resource,'-n',system,'--as='+operator],ok=False)
            assert allowed.stdout.strip()=='yes','native controller cannot patch system '+resource+' metadata'
            for namespace,account in [(app,operator),(system,'system:serviceaccount:'+system+':'+installation+'-admission')]:
                denied=kubectl(['auth','can-i','patch',resource,'-n',namespace,'--as='+account],ok=False)
                assert denied.stdout.strip()=='no','native metadata permission escaped its identity or system namespace'
        check('native metadata patch permissions are limited to the operator system namespace')
        # Build output is private attempt data, not disposable Pod stdout or
        # public release metadata. Exercise the real native BuildKit path.
        failing=image('diagnostic-failure')
        failing['spec']['platforms']=[dict(os='linux',architecture=arch)]
        failing['spec']['source']['build']['definition']['inline']='FROM scratch\nARG TARGETARCH\nCOPY hello-${TARGETARCH} /probe\nRUN ["/probe", "diagnose"]\n'
        create(failing)
        def attempts():
            result=run([args.installer.resolve(),'builds','diagnostic-failure','--namespace',app],ok=False)
            return json.loads(result.stdout) if result.returncode==0 else {}
        active=wait(lambda: attempts().get('active'), 'diagnostic client starts', 120)
        attempt=active[0]['attempt']
        followed=run([args.installer.resolve(),'logs','diagnostic-failure','--namespace',app,'--platform','linux/'+arch,'--attempt',attempt,'--follow'],timeout=180)
        retained=wait(lambda: attempts().get('retained'), 'private log archived before cleanup', 180)
        archived=run([args.installer.resolve(),'logs','diagnostic-failure','--namespace',app,'--attempt',attempt])
        log_assertions.verify(followed.stdout, archived.stdout, retained, attempt)
        public=json.dumps(get('images.kimage.dev'))+json.dumps(get('imagereleases.kimage.dev'))
        assert 'kimage-diagnostic-output-' not in public and 'kimage-diagnostic-failure-end' not in public
        logs=[v for v in get('secrets')['items'] if v['metadata'].get('labels',{}).get('kimage.dev/build-log')==get('images.kimage.dev','diagnostic-failure')['metadata']['uid']]
        assert len(logs)==1 and logs[0]['immutable'] and not logs[0]['metadata']['labels'].get('kimage.dev/build-snapshot')
        assert logs[0]['metadata']['labels'].get('kimage.dev/build-state')==get('images.kimage.dev','diagnostic-failure')['metadata']['uid']
        denied=kubectl(['auth','can-i','get','secrets','-n',app,'--as=system:serviceaccount:e2e-system:e2e-admission'],ok=False)
        assert denied.stdout.strip()=='no'
        check('native failed build streams output and archives a bounded private tail after Pod cleanup')

        encoded = create(pod('platform://'+app+'/service?version=%5E1.0.0', 'encoded'), dry_run=True)
        assert json.loads(encoded.stdout)['spec']['containers'][0]['image'].endswith('@'+release['spec']['root']['digest'])
        check('readable and percent-encoded selectors resolve the same native digest')
        wrong_platform = pod(selector, 'wrong-platform')
        wrong_platform['spec']['nodeSelector']['kubernetes.io/arch'] = 'arm64' if arch == 'amd64' else 'amd64'
        rejected = create(wrong_platform, dry_run=True, ok=False)
        assert rejected.returncode, 'unavailable native architecture unexpectedly admitted'
        assert len([v for v in get('images.kimage.dev')['items'] if v['metadata'].get('labels', {}).get('kimage.dev/platform-parent') == built['metadata']['uid']]) == 1
        check('unavailable platform placement is rejected without a fallback build')
        channel_update = get('images.kimage.dev', 'service')
        channel_update['spec']['channels'] = {'stable':'1.0.0'}
        kubectl(['replace','-f','-'], channel_update)
        retained = wait(ready, 'channel update acknowledges retained native artifact')
        assert retained['status']['releaseRef'] == built['status']['releaseRef']
        assert len([v for v in get('images.kimage.dev')['items'] if v['metadata'].get('labels', {}).get('kimage.dev/platform-parent') == built['metadata']['uid']]) == 1
        check('channel changes retain native readiness without new build demand')
        # Exercise eight built-in/custom controller Pod templates, including init
        # containers and native sidecars, through the real admission server.
        run(['python3', ROOT/'scripts/live-workload-consumers.py', '--kubeconfig', kubeconfig, '--namespace', app, '--image', 'service', '--ci-owned-cluster', '--output', args.output/'controllers.json'], timeout=900)
        check('built-in and custom controllers, init containers and sidecars use native releases')
        foreign = create(pod(selector, 'foreign'), ns=other, dry_run=True, ok=False)
        if args.scope == 'cluster':
            assert foreign.returncode, 'cross-namespace selector unexpectedly admitted'
            create(image(), ns=other)
            local_other = 'platform://'+other+'/service:^1.0.0'
            other_deployment = copy.deepcopy(deployment)
            other_deployment['spec']['template']['spec']['containers'][0]['image'] = local_other
            create(other_deployment, ns=other)
            second = wait(lambda: ready(ns=other), 'second namespace native build', 600)
            create(pod(local_other, 'local-boundary-control'), ns=other, dry_run=True)
            assert second['metadata']['uid'] != built['metadata']['uid']
            run(command+['rollout', 'status', 'deployment/consumer', '-n', other, '--timeout=300s'], timeout=320)
            assert get('imagereleases.kimage.dev', second['status']['releaseRef'], other)['spec']['imageUid'] == second['metadata']['uid']
            untouched = create(pod('platform://'+excluded+'/missing:1.0.0'), ns=excluded, dry_run=True)
            assert json.loads(untouched.stdout)['spec']['containers'][0]['image'].startswith('platform://')
            check('cluster scope handles independent namespaces, rejects cross-namespace consumption and skips exclusions')
        else:
            assert foreign.returncode == 0 and json.loads(foreign.stdout)['spec']['containers'][0]['image'] == selector
            check('namespace scope leaves other namespaces untouched')
        # No catalog writes during the old transport's 30-second idle window.
        # A fresh Image must still be noticed without restarting the operator.
        time.sleep(35)
        create(image('eager'))
        wait(lambda: any(c.get('reason')=='AwaitingDemand' for c in (get('images.kimage.dev', 'eager') or {}).get('status', {}).get('conditions', [])), 'eager candidate initially awaits demand')
        check('quiet catalog watch observes a fresh Image after thirty seconds')
        config = get('configmap', 'e2e-config', system)
        config['data']['configuration.json'] = json.dumps(dict(schema=1, buildPolicy='Eager'))
        kubectl(['replace', '-f', '-'], config)
        wait(lambda: ready('eager'), 'eager native build without consumer', 600)
        assert all(v['spec']['imageName'] != 'eager' for v in get('imagedemands.kimage.dev', ns=system)['items'])
        check('administrator ConfigMap eager policy builds without workload demand')
        again=install()
        assert again.returncode==0, 'idempotent reapply failed: '+again.stderr[-3000:]
        assert json.loads(get('configmap','e2e-config',system)['data']['configuration.json'])['buildPolicy']=='Eager'
        assert all(get('secret',name,system)['metadata']['uid']==uid for name,uid in credential_uids.items())
        assert get('pvc','e2e-data',system)['metadata']['uid']==pvc_uid
        check('reapply preserves credentials, registry storage and administrator eager policy')
        if args.mode=='cli':
            run([args.installer.resolve(),'uninstall','--namespace',system,'--installation',installation],timeout=300)
        else:
            run(['helm','uninstall',installation,'--namespace',system,'--wait','--timeout','5m'],timeout=320)
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
        for role in ['operator','admission']:
            result = kubectl(['logs', 'deployment/e2e-'+role, '-n', system, '--tail=200', '--timestamps'], ok=False)
            (args.output/(role+'.log')).write_text(result.stdout+result.stderr)
        for container in ['gateway', 'registry']:
            result = kubectl(['logs', 'deployment/e2e-registry', '-n', system, '-c', container, '--tail=100', '--timestamps'], ok=False)
            (args.output/('registry-'+container+'.log')).write_text(result.stdout+result.stderr)
        (args.output/'results.json').write_text(json.dumps(report, indent=2)+'\n')
    print('E2E complete: '+args.scope+' linux/'+arch)


if __name__ == '__main__':
    main()

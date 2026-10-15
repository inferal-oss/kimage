#!/usr/bin/env python3
"""Exercise built-in controllers and a small custom-controller fixture.

Only runs against the task-owned local QEMU acceptance cluster. Uses an already
published scripts/fixtures/build-probe.rs image, creates bounded uniquely labelled workloads, and
removes every created object (including the temporary fixture CRD) in finally.
It never updates the installation, its images, or its release catalog.
"""
import argparse
import copy
import json
import pathlib
import re
import subprocess
import time
import uuid

ROOT = pathlib.Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--kubeconfig', type=pathlib.Path, required=True)
    parser.add_argument('--namespace', default='kimage-app')
    parser.add_argument('--image', default='kimage-matrix-a1-cached',
                        help='published Image using scripts/fixtures/build-probe.rs, including its wait mode')
    parser.add_argument('--selector-format', choices=['readable', 'legacy-query'], default='readable',
                        help='legacy-query is only for retained fixtures predating readable colon ranges')
    parser.add_argument('--output', type=pathlib.Path, required=True)
    args = parser.parse_args()
    assert re.search(r'server: https://127\.0\.0\.1:27643\s', args.kubeconfig.read_text())
    assert args.namespace == 'kimage-app', 'fixture application scope required'
    command = ['kubectl', '--kubeconfig', str(args.kubeconfig), '--request-timeout=15s']
    prefix = 'kimage-consumers-' + uuid.uuid4().hex[:8]
    label = {'kimage.dev/consumer-test': prefix}
    created = []
    receipt = {'cases': [], 'cleanupComplete': False}

    def call(arguments, value=None):
        result = subprocess.run(command + arguments, input=None if value is None else json.dumps(value),
                                text=True, capture_output=True, timeout=25)
        if result.returncode:
            raise RuntimeError(result.stderr[:2000])
        return json.loads(result.stdout) if result.stdout.strip().startswith('{') else result.stdout

    def get(resource, name):
        return call(['-n', args.namespace, 'get', resource, name, '-o', 'json'])

    def create(body):
        result = call(['create', '-f', '-', '-o', 'json'], body)
        created.append((result['apiVersion'], result['kind'], result['metadata']['name'],
                        result['metadata'].get('namespace'), result['metadata']['uid']))
        return result

    def template_at(body, pointer):
        for field in pointer.strip('/').split('/'):
            body = body[field]
        return body

    def save():
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(receipt, indent=2) + '\n')

    server = call(['config', 'view', '--minify', '-o', 'jsonpath={.clusters[0].cluster.server}'])
    assert server == 'https://127.0.0.1:27643', 'current context must select the owned fixture'

    try:
        image = get('images.kimage.dev', args.image)
        release = get('imagereleases.kimage.dev', image['status']['releaseRef'])
        assert release['status']['verified']
        assert release['status']['observedGeneration'] == release['metadata']['generation']
        expected_digest = release['spec']['root']['digest']
        selector = 'platform://' + args.namespace + '/' + args.image + ':^1.0.0'
        if args.selector_format == 'legacy-query':
            selector = 'platform://' + args.namespace + '/' + args.image + '?version=%5E1.0.0'
        receipt['selector'] = selector
        receipt['selectorFormat'] = args.selector_format
        receipt['expectedDigest'] = expected_digest
        group = prefix + '.fixtures.kimage.dev'
        crd_name = 'fixtureworkloads.' + group
        create({'apiVersion': 'apiextensions.k8s.io/v1', 'kind': 'CustomResourceDefinition',
                'metadata': {'name': crd_name, 'labels': label},
                'spec': {'group': group, 'scope': 'Namespaced',
                         'names': {'kind': 'FixtureWorkload', 'plural': 'fixtureworkloads'},
                         'versions': [{'name': 'v1', 'served': True, 'storage': True,
                                       'schema': {'openAPIV3Schema': {'type': 'object', 'properties': {
                                           'spec': {'type': 'object', 'properties': {
                                               'task': {'type': 'object', 'x-kubernetes-preserve-unknown-fields': True}}}}}}}]}})
        deadline = time.monotonic() + 30
        while True:
            definition = get('crd', crd_name)
            if any(c['type'] == 'Established' and c['status'] == 'True'
                   for c in (definition.get('status', {}).get('conditions') or [])):
                break
            assert time.monotonic() < deadline, 'fixture CRD did not become established'
            time.sleep(1)
        matrix = json.loads((ROOT / 'crates/control/tests/fixtures/workload-consumers.json').read_text())
        for case in matrix:
            kind = case['kind']
            name = prefix + '-' + kind.lower()
            labels = dict(label, app=name)
            container = {'name': 'app', 'image': selector,
                         'resources': {'requests': {'cpu': '5m', 'memory': '8Mi'},
                                       'limits': {'cpu': '100m', 'memory': '32Mi'}}}
            job = kind in ('Job', 'CronJob')
            if job:
                container['args'] = ['wait', '1']
            init = dict(copy.deepcopy(container), name='init', args=['wait', '0'])
            sidecar = copy.deepcopy(container)
            sidecar.update(name='sidecar', args=[], restartPolicy='Always')
            template = {'metadata': {'labels': labels}, 'spec': {
                'nodeSelector': {'kubernetes.io/hostname': 'kimage-clean'},
                'terminationGracePeriodSeconds': 1,
                'automountServiceAccountToken': False,
                'restartPolicy': 'Never' if job else 'Always',
                'containers': [container], 'initContainers': [init, sidecar]}}
            body = {'apiVersion': case['apiVersion'], 'kind': kind,
                    'metadata': {'name': name, 'namespace': args.namespace, 'labels': labels}, 'spec': {}}
            cursor = body
            fields = case['template'].strip('/').split('/')
            for field in fields[:-1]:
                cursor = cursor.setdefault(field, {})
            cursor[fields[-1]] = template
            if kind in ('Deployment', 'StatefulSet', 'ReplicaSet', 'ReplicationController'):
                body['spec']['replicas'] = 1
            if kind in ('Deployment', 'StatefulSet', 'ReplicaSet', 'DaemonSet'):
                body['spec']['selector'] = {'matchLabels': {'app': name}}
            if kind == 'ReplicationController':
                body['spec']['selector'] = {'app': name}
            if kind == 'StatefulSet':
                body['spec']['serviceName'] = name
            if kind == 'CronJob':
                body['spec'].update(schedule='* * * * *', concurrencyPolicy='Forbid',
                                    successfulJobsHistoryLimit=1, failedJobsHistoryLimit=1)
            if kind == 'FixtureWorkload':
                body['apiVersion'] = group + '/v1'
            stored = create(body)
            assert template_at(stored, case['template'])['spec']['containers'][0]['image'] == selector
            if kind == 'FixtureWorkload':
                # The fixture's controller reconciliation: read its nonstandard
                # template location, copy it unchanged into an owned Pod CREATE.
                source = get('fixtureworkloads.' + group, name)
                pod = copy.deepcopy(template_at(source, case['template']))
                pod.update(apiVersion='v1', kind='Pod')
                pod['metadata'].update(name=name, namespace=args.namespace, ownerReferences=[{
                    'apiVersion': source['apiVersion'], 'kind': kind, 'name': name,
                    'uid': source['metadata']['uid'], 'controller': True}])
                create(pod)
            deadline = time.monotonic() + 110
            while True:
                pods = call(['-n', args.namespace, 'get', 'pods', '-l', 'app=' + name, '-o', 'json'])['items']
                ready = []
                for pod in pods:
                    spec = pod['spec']
                    entries = spec['containers'] + spec.get('initContainers', [])
                    assert all(c['image'].endswith('@' + expected_digest) for c in entries)
                    assert all(c['imagePullPolicy'] == 'IfNotPresent' for c in entries)
                    records = json.loads(pod['metadata']['annotations']['kimage.dev/resolutions'])
                    assert len(records) == 3 and all(r['selector'] == selector for r in records)
                    status = pod.get('status', {})
                    states = status.get('containerStatuses', []) + status.get('initContainerStatuses', [])
                    if len(states) == 3 and all(
                            'running' in c['state']
                            or c['state'].get('terminated', {}).get('exitCode') == 0
                            or (c['name'] == 'sidecar' and status.get('phase') == 'Succeeded'
                                and 'terminated' in c['state']) for c in states):
                        ready.append(pod)
                if ready:
                    break
                if time.monotonic() >= deadline:
                    raise RuntimeError(kind + ': generated Pod did not start successfully')
                time.sleep(2)
            # The API must still store selectors in the parent after admission.
            current = get(kind if kind != 'FixtureWorkload' else 'fixtureworkloads.' + group, name)
            assert template_at(current, case['template'])['spec']['containers'][0]['image'] == selector
            receipt['cases'].append({'kind': kind, 'templatePreserved': True,
                                     'podDigestPinned': True, 'normalInitAndSidecarStarted': True,
                                     'podNames': [p['metadata']['name'] for p in ready]})
            save()
            print(kind + ': symbolic template, digest-pinned Pod, all containers started', flush=True)
    finally:
        cleanup_errors = []
        for version, kind, name, namespace, uid in reversed(created):
            if kind == 'CustomResourceDefinition':
                path = '/apis/apiextensions.k8s.io/v1/customresourcedefinitions/' + name
            elif version == 'v1':
                path = '/api/v1/namespaces/' + namespace + '/' + kind.lower() + 's/' + name
            else:
                path = '/apis/' + version + '/namespaces/' + namespace + '/' + kind.lower() + 's/' + name
            try:
                call(['delete', '--raw', path, '-f', '-'], {
                    'apiVersion': 'v1', 'kind': 'DeleteOptions', 'propagationPolicy': 'Foreground',
                    'preconditions': {'uid': uid}})
            except Exception as error:
                cleanup_errors.append(str(error))
        try:
            call(['-n', args.namespace, 'wait', '--for=delete', 'pods',
                  '-l', 'kimage.dev/consumer-test=' + prefix, '--timeout=20s'])
        except Exception as error:
            cleanup_errors.append(str(error))
        receipt['cleanupComplete'] = not cleanup_errors
        receipt['cleanupErrors'] = cleanup_errors
        save()
        assert not cleanup_errors, cleanup_errors
    print('All consumer fixtures removed.', flush=True)


if __name__ == '__main__':
    main()

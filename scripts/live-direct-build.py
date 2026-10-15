#!/usr/bin/env python3
"""Exercise the public Direct build API on the owned disconnected fixture."""
import argparse
import base64
import json
import os
import pathlib
import subprocess
import time

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--kubeconfig', type=pathlib.Path, required=True)
p.add_argument('--probe', type=pathlib.Path, required=True)
p.add_argument('--output', type=pathlib.Path, required=True)
p.add_argument('--case', required=True)
p.add_argument('--node', choices=['kimage-clean', 'kimage-clean-2'])
a = p.parse_args()
assert 'server: https://127.0.0.1:27643' in a.kubeconfig.read_text()
assert a.case.startswith('kimage-direct-') and a.case.replace('-', '').isalnum()
a.output.mkdir(parents=True, exist_ok=True)
base = ['kubectl', '--kubeconfig', str(a.kubeconfig), '--request-timeout=45s', '-n', 'kimage-app']


def call(args, body=None):
    r = subprocess.run(base + args, input=None if body is None else json.dumps(body).encode(), capture_output=True, timeout=60)
    if r.returncode:
        raise RuntimeError(r.stderr.decode()[:2000])
    return r.stdout


def get(kind, name):
    return json.loads(call(['get', kind, name, '-o', 'json']))


def create(body):
    existing = call(['get', body['kind'], body['metadata']['name'], '--ignore-not-found', '-o', 'name'])
    if not existing:
        call(['create', '-f', '-'], body)


def wait_ready(kind, name, seconds=900):
    deadline = time.monotonic() + seconds
    last = None
    while time.monotonic() < deadline:
        v = get(kind, name)
        conditions = v.get('status', {}).get('conditions', [])
        phase = [(c['type'], c['status'], c.get('reason')) for c in conditions]
        if phase != last:
            print(kind, name, phase, flush=True)
            last = phase
        if (kind != 'image' or v.get('status', {}).get('observedGeneration') == v['metadata']['generation']) and any(c['type'] == 'Ready' and c['status'] == 'True' for c in conditions):
            return v
        time.sleep(3)
    raise RuntimeError('readiness deadline: ' + name)


probe = a.probe.read_bytes()
assert len(probe) <= 512 * 1024
source = a.case + '-input'
create(dict(apiVersion='v1', kind='ConfigMap', metadata=dict(name=source),
            data=dict(message=a.case), binaryData=dict(executable=base64.b64encode(probe).decode())))
definition = 'FROM scratch\nCOPY bin/probe /probe\nCOPY message /message\nENTRYPOINT ["/probe"]\n'
build = dict(format='Containerfile', definition=dict(inline=definition),
             execution=dict(strategy='Direct', builderRef='default'), cache=dict(mode='Local'),
             inputs=dict(contextPath='/workspace/context',
                         volumes=[dict(name='source', configMap=dict(name=source, items=[
                             dict(key='executable', path='bin/probe', mode=365),
                             dict(key='message', path='message', mode=292)]))],
                         volumeMounts=[dict(name='source', mountPath='/workspace/context', readOnly=True)]))
create(dict(apiVersion='kimage.dev/v1alpha1', kind='Image', metadata=dict(name=a.case),
            spec=dict(version='1.0.0', platform=dict(os='linux', architecture='amd64'), source=dict(build=build))))
image = wait_ready('image', a.case)
release = get('imagerelease', image['status']['releaseRef'])
assert release['status']['verified']
execution = release['spec']['provenance']['execution']
assert execution['engine'] == 'BuildKit' and execution['engineVersion'] == '0.33.0'
assert execution['engineWorkerId'] and execution['engineOperationId']
pod_name = a.case + '-workload'
pod_spec = dict(
    restartPolicy='Never', automountServiceAccountToken=False,
    containers=[dict(name='app', image='platform://kimage-app/' + a.case + ':1.0.0', imagePullPolicy='IfNotPresent',
                     resources=dict(requests=dict(cpu='5m', memory='8Mi'), limits=dict(cpu='100m', memory='32Mi')))])
if a.node:
    pod_spec['nodeSelector'] = {'kubernetes.io/hostname':a.node}
create(dict(apiVersion='v1', kind='Pod', metadata=dict(name=pod_name), spec=pod_spec))
pod = wait_ready('pod', pod_name, 240)
assert pod['spec']['containers'][0]['image'].endswith('@' + release['spec']['root']['digest'])
assert call(['logs', pod_name]).decode().strip() == 'kimage normal pull'
proxies = json.loads(subprocess.check_output(base[:-2] + ['-n', 'kimage-system', 'get', 'pods', '-l', 'kimage.dev/installation=kimage-clean', '-o', 'json'], timeout=60))['items']
proxy = next(v['metadata']['name'] for v in proxies if v['spec'].get('nodeName') == pod['spec']['nodeName'] and any(c['name'] == 'proxy' for c in v['spec']['containers']) and not v['metadata'].get('deletionTimestamp'))
proxy_logs = subprocess.check_output(base[:-2] + ['-n', 'kimage-system', 'logs', proxy, '--tail=2000'], timeout=60).decode()
assert release['spec']['root']['digest'] in proxy_logs, 'fresh root request absent from proxy receipt'
evidence = dict(runtimeRootRequestObserved=True, imageUid=image['metadata']['uid'], releaseUid=release['metadata']['uid'],
                root=release['spec']['root'], execution=execution, workloadUid=pod['metadata']['uid'],
                workloadReady=True, configMapBinaryMapping=True, inlineDefinition=True)
(a.output / 'evidence.json').write_text(json.dumps(evidence, indent=2) + '\n')
print('public Direct API build and normal resolved workload pull passed', flush=True)

#!/usr/bin/env python3
"""Negative public build-API acceptance on the owned disconnected fixture."""
import argparse
import copy
import json
import hashlib
import os
import pathlib
import subprocess
import time

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--kubeconfig', type=pathlib.Path, required=True)
p.add_argument('--output', type=pathlib.Path, required=True)
p.add_argument('--case', required=True)
a = p.parse_args()
assert 'server: https://127.0.0.1:27643' in a.kubeconfig.read_text()
assert a.case.startswith('kimage-invalid-') and a.case.replace('-', '').isalnum()
a.output.parent.mkdir(parents=True, exist_ok=True)
b = ['kubectl', '--kubeconfig', str(a.kubeconfig), '--request-timeout=45s', '-n', 'kimage-app']


def call(args, body=None):
    return subprocess.run(b + args, input=None if body is None else json.dumps(body).encode(), capture_output=True, timeout=65)


source = a.case + '-input'
r = call(['get', 'configmap', source, '--ignore-not-found', '-o', 'name'])
assert r.returncode == 0
if not r.stdout:
    r = call(['create', '-f', '-'], dict(apiVersion='v1', kind='ConfigMap', metadata=dict(name=source), data=dict(ordinary='fixture')))
    assert r.returncode == 0
build = dict(format='Containerfile', execution=dict(strategy='Direct', builderRef='default'),
             definition=dict(inline='FROM scratch\nCOPY ordinary /ordinary\n'),
             inputs=dict(contextPath='/workspace/context', volumes=[dict(name='context', configMap=dict(name=source))],
                         volumeMounts=[dict(name='context', mountPath='/workspace/context', readOnly=True)]))
baseline = dict(apiVersion='kimage.dev/v1alpha1', kind='Image', metadata=dict(name='replaced'), spec=dict(
    version='1.0.0', platform=dict(os='linux', architecture='amd64'), source=dict(build=build)))
cases = {}


def case(label, transform, reason=None):
    v = copy.deepcopy(baseline)
    v['metadata']['name'] = a.case + '-' + label
    transform(v)
    cases[label] = (v, reason)


case('format', lambda v: v['spec']['source']['build'].update(format='Shell'))
case('strategy', lambda v: v['spec']['source']['build']['execution'].update(strategy='Remote'))
case('builder', lambda v: v['spec']['source']['build']['execution'].update(builderRef='unapproved'))
case('platform', lambda v: v['spec']['platform'].update(architecture='arm64'))
case('definition-union', lambda v: v['spec']['source']['build']['definition'].update(fromFile=dict(path='/workspace/inputs/recipe')))
case('source-union', lambda v: v['spec']['source'].update(ociArchive=dict(claimName='source', path='input.tar', producerHandoff=True, root=dict(mediaType='application/vnd.oci.image.manifest.v1+json', digest='sha256:' + '0' * 64, size=1))))
case('host-path', lambda v: v['spec']['source']['build']['inputs']['volumes'][0].update(hostPath=dict(path='/')))
case('mount-expression', lambda v: v['spec']['source']['build']['inputs']['volumeMounts'][0].update(subPathExpr='POD_NAME'))
case('escape', lambda v: v['spec']['source']['build']['inputs'].update(contextPath='/workspace/context/../escape'), 'UnsafeInputPath')
case('overlap', lambda v: v['spec']['source']['build']['inputs']['volumeMounts'].append(dict(name='context', mountPath='/workspace/context/nested', readOnly=True)), 'OverlappingInputMounts')
case('missing-key', lambda v: v['spec']['source']['build']['inputs']['volumes'][0]['configMap'].update(items=[dict(key='absent', path='file')]), 'MissingInputKey')
case('missing-file', lambda v: v['spec']['source']['build'].update(definition=dict(fromFile=dict(path='/workspace/context/absent'))), 'MissingInputFile')
case('frontend', lambda v: v['spec']['source']['build'].update(definition=dict(inline='# syntax=example.invalid/frontend\nFROM scratch\n')), 'ExternalFrontendDirectiveDenied')
private_path = a.output.with_suffix('.private.json')
if not private_path.exists():
    private_path.write_text(json.dumps(dict(marker='KIMAGESECRET' + os.urandom(16).hex().upper())))
    private_path.chmod(0o600)
marker = json.loads(private_path.read_text())['marker']
private_definition = 'FROM scratch\n' + marker + ' invalid\n'
secret_name = a.case + '-private'
if not call(['get', 'secret', secret_name, '--ignore-not-found', '-o', 'name']).stdout:
    r = call(['create', '-f', '-'], dict(apiVersion='v1', kind='Secret', metadata=dict(name=secret_name), type='Opaque', stringData=dict(recipe=private_definition)))
    assert r.returncode == 0


def invalid_secret(v):
    build = v['spec']['source']['build']
    build['definition'] = dict(fromFile=dict(path='/run/kimage-build-secrets/recipe'))
    build['inputs']['volumes'].append(dict(name='private', secret=dict(name=secret_name)))
    build['inputs']['volumeMounts'].append(dict(name='private', mountPath='/run/kimage-build-secrets', readOnly=True))


case('secret-definition', invalid_secret, 'ContainerfileSolveFailed')
results = json.loads(a.output.read_text()) if a.output.exists() else {}
for label, (body, reason) in cases.items():
    if label in results:
        continue
    name = body['metadata']['name']
    prior = call(['get', 'image', name, '--ignore-not-found', '-o', 'json'])
    assert prior.returncode == 0
    r = call(['create', '-f', '-'], body) if not prior.stdout else prior
    if r.returncode:
        error = r.stderr.decode()
        assert 'Invalid' in error or 'invalid' in error, error[:500]
        results[label] = dict(schemaRefused=True)
    else:
        assert reason, label + ' unexpectedly passed schema validation'
        end = time.monotonic() + 240
        while time.monotonic() < end:
            r = call(['get', 'image', name, '-o', 'json'])
            assert r.returncode == 0
            image = json.loads(r.stdout)
            conditions = image.get('status', {}).get('conditions', [])
            assert not any(c['type'] == 'Ready' and c['status'] == 'True' for c in conditions)
            if any(c['type'] == 'Ready' and c['status'] == 'False' and c.get('reason') == reason for c in conditions):
                results[label] = dict(controllerRefused=True, reason=reason, imageUid=image['metadata']['uid'])
                break
            time.sleep(2)
        else:
            raise RuntimeError(label + ' did not report ' + reason)
    a.output.write_text(json.dumps(results, indent=2) + '\n')
    print('negative build case', label, 'passed', flush=True)
if not results['secret-definition'].get('privateDefinitionAndFingerprintAbsentFromStatusAndDaemonLogs'):
    response = call(['get', 'image', a.case + '-secret-definition', '-o', 'json'])
    assert response.returncode == 0
    public = response.stdout
    assert marker.encode() not in public
    assert hashlib.sha256(private_definition.encode()).hexdigest().encode() not in public
    for pod in ['kimage-clean-operator', 'kimage-clean-builder']:
        r = subprocess.run(b[:-2] + ['-n', 'kimage-system', 'logs', pod, '--tail=3000'], capture_output=True, timeout=65)
        assert r.returncode == 0 and marker.encode() not in r.stdout + r.stderr
    results['secret-definition']['privateDefinitionAndFingerprintAbsentFromStatusAndDaemonLogs'] = True
a.output.write_text(json.dumps(results, indent=2) + '\n')
print('negative build declarations and mounted-input failures refused', flush=True)

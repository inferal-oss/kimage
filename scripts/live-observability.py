#!/usr/bin/env python3
"""Verify aggregate metrics against a real build, publication and normal pull."""
import argparse
import contextlib
import http.client
import json
import pathlib
import socket
import ssl
import subprocess
import time

p = argparse.ArgumentParser(description=__doc__)
for key in ['kubeconfig','probe','admission-ca','output']:
    p.add_argument('--' + key,type=pathlib.Path,required=True)
p.add_argument('--case',required=True)
a = p.parse_args()
assert a.case.startswith('kimage-direct-metrics-') and a.case.replace('-','').isalnum()
assert 'server: https://127.0.0.1:27643' in a.kubeconfig.read_text()
a.output.mkdir(parents=True,exist_ok=True)
b = ['kubectl','--kubeconfig',str(a.kubeconfig),'--request-timeout=45s']


@contextlib.contextmanager
def forward(pod,port,remote):
    with socket.socket() as probe:
        probe.bind(('127.0.0.1',port))
    with (a.output/(pod+'-forward.log')).open('wb') as log:
        child = subprocess.Popen(b+['-n','kimage-system','port-forward','--address=127.0.0.1','pod/'+pod,str(port)+':'+str(remote)],stdout=log,stderr=subprocess.STDOUT)
        try:
            deadline=time.monotonic()+30
            while time.monotonic()<deadline:
                assert child.poll() is None,'API forwarding exited'
                try:
                    with socket.create_connection(('127.0.0.1',port),timeout=1):break
                except OSError:time.sleep(.2)
            else:raise RuntimeError('API forwarding deadline')
            yield
        finally:
            child.terminate()
            child.wait(timeout=10)


def scrape(port,authority=None,ca=None):
    conn=http.client.HTTPConnection('127.0.0.1',port,timeout=10)
    if ca:
        conn.sock=ssl.create_default_context(cafile=str(ca)).wrap_socket(socket.create_connection(('127.0.0.1',port),timeout=10),server_hostname=authority)
    conn.request('GET','/metrics',headers={'Host':authority or '127.0.0.1:'+str(port)})
    response=conn.getresponse()
    assert response.status==200
    body=response.read(16385).decode()
    conn.close()
    assert len(body)<16384
    lines=[line for line in body.splitlines() if line and not line.startswith('#')]
    assert all('namespace=' not in line and 'digest=' not in line and 'uid=' not in line for line in lines)
    return {line.rsplit(' ',1)[0]:float(line.rsplit(' ',1)[1]) for line in lines}


with forward('kimage-clean-operator',28780,8080),forward('kimage-clean-proxy',28781,17641),forward('kimage-clean-admission',28782,8443):
    before=dict(operator=scrape(28780),proxy=scrape(28781,'127.0.0.1:17641'))
    subprocess.run(['python3','scripts/live-direct-build.py','--kubeconfig',str(a.kubeconfig),'--probe',str(a.probe),'--case',a.case,'--node','kimage-clean','--output',str(a.output/'build')],check=True,timeout=1000)
    sampled_after = before['operator']['kimage_catalog_sample_timestamp_seconds'] + 1
    images = json.loads(subprocess.check_output(b+['-n','kimage-app','get','images','-o','json'],timeout=60))['items']
    releases = json.loads(subprocess.check_output(b+['-n','kimage-app','get','imagereleases','-o','json'],timeout=60))['items']
    images = [v for v in images if not v['metadata'].get('deletionTimestamp')]
    ready_count = sum(v.get('status',{}).get('observedGeneration')==v['metadata']['generation'] and any(c['type']=='Ready' and c['status']=='True' for c in v.get('status',{}).get('conditions',[])) for v in images)
    verified_count = sum(not v['metadata'].get('deletionTimestamp') and v.get('status',{}).get('verified') is True for v in releases)
    deadline=time.monotonic()+45
    while time.monotonic()<deadline:
        operator=scrape(28780)
        if operator['kimage_catalog_sample_complete']==1 and operator['kimage_catalog_sample_timestamp_seconds']>=sampled_after and operator['kimage_images_ready']==ready_count and operator['kimage_images_pending']==len(images)-ready_count and operator['kimage_releases_verified']==verified_count and operator['kimage_reconcile_waiting']==0:break
        time.sleep(1)
    else:raise RuntimeError('catalog metrics sample deadline')
    after=dict(operator=operator,proxy=scrape(28781,'127.0.0.1:17641'),admission=scrape(28782,'kimage-clean-admission.kimage-system.svc',a.admission_ca))
    for component,stage in [('operator','build'),('operator','publication'),('proxy','pull')]:
        for metric in ['kimage_stage_duration_seconds_count','kimage_stage_duration_seconds_sum','kimage_stage_transferred_bytes_total']:
            key=metric+'{stage="'+stage+'"}'
            assert after[component][key]>before[component][key],key+' did not observe the real operation'
    assert after['admission']['kimage_nodes_eligible']==2
    assert after['admission']['kimage_node_sample_timestamp_seconds']>0
    assert after['operator']['kimage_reconcile_waiting']==0
    assert after['operator']['kimage_catalog_sample_timestamp_seconds']>0
result=dict(realBuildPublicationPullMetricsIncreased=True,completeCatalogSample=True,twoEligibleNodesObserved=True,noIdentityMetricLabels=True,boundedOutput=True,before=before,after=after)
(a.output/'evidence.json').write_text(json.dumps(result,indent=2)+'\n')
print('actual build/publication/pull latency and byte metrics, catalog and node readiness passed',flush=True)

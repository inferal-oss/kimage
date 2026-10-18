#!/usr/bin/env python3
"""Check the independent future chart retains ordinary Helm and Argo resources."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
ROOT=Path(__file__).resolve().parents[1]
def module(name):
    spec=importlib.util.spec_from_file_location(name,ROOT/'scripts'/(name+'.py'));value=importlib.util.module_from_spec(spec);spec.loader.exec_module(value);return value
fixture=module('prepare-argocd-next');baseline=module('check-chart');gitops=module('test-gitops-chart')
def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--helm',default='helm');args=parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='kimage-forward-chart-') as temporary:
        work=Path(temporary);chart=work/'chart';fixture.chart(chart)
        environment=dict(os.environ,KUBECONFIG=str(work/'no-cluster-access'))
        for scope in ['namespace','cluster']:
            for argo in [False,True]:
                values=dict(installation='future',scope=scope,applicationNamespace='apps' if scope=='namespace' else '',excludedNamespaces=['excluded'] if scope=='cluster' else [],gitops=dict(enabled=argo),images=dict(digests={r:'sha256:'+str(i+1)*64 for i,r in enumerate(baseline.ROLES)}))
                if not argo:values['nodes']=[baseline.node('node-'+arch,arch) for arch in ['amd64','arm64']]
                path=work/'values.json';path.write_text(json.dumps(values))
                result=subprocess.check_output([args.helm,'template','future',str(chart),'--namespace','future-system','-f',str(path)],env=environment,text=True)
                objects=gitops.documents(result);by_name={(o['kind'],o['metadata']['name']):o for o in objects}
                if argo:
                    assert sorted(o['kind'] for o in objects)==['ClusterRole','ClusterRoleBinding','ConfigMap','Deployment','ServiceAccount']
                    catalog=json.loads(by_name['ConfigMap','future-provisioner']['data']['desired.json'])['catalog']
                else:
                    for role in ['registry','admission','builder']:
                        assert by_name['Secret','future-'+role+'-credentials']['data'],'Missing ordinary Helm credentials'
                    for role in ['operator','admission']:assert ('Deployment','future-'+role) in by_name
                    assert by_name['PersistentVolumeClaim','future-data']['metadata']['annotations']['helm.sh/resource-policy']=='keep'
                    assert all(('Pod','future-builder-'+arch) in by_name for arch in ['amd64','arm64'])
                    catalog=json.loads(by_name['ConfigMap','future-component-images']['data']['images.json'])
                assert catalog['release']==fixture.VERSION and catalog['apiSchemaRevision']==7
                assert all(catalog['components'][role]['version']==fixture.VERSION for role in ['admission','operator','input','exporter'])
                print('PASS: future '+scope+' '+('Argo' if argo else 'Helm')+' chart retains the complete intended resource set and schema-7 catalog')
if __name__=='__main__':main()

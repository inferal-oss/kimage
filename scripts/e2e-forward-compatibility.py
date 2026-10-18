#!/usr/bin/env python3
"""Explicit disposable CLI/Helm future-schema rollback on real native workers.

Never point this fixture at a shared production API: schema revision 7 remains
installed. Mixed production-cluster predecessor round trips use e2e-upgrade.py.
"""
import argparse
import copy
import hashlib
import importlib.util
import json
from pathlib import Path
import shutil

ROOT=Path(__file__).resolve().parents[1]
def module(name):
    spec=importlib.util.spec_from_file_location(name,ROOT/'scripts'/(name+'.py'))
    value=importlib.util.module_from_spec(spec);spec.loader.exec_module(value);return value
upgrade=module('e2e-upgrade');future_data=module('e2e-forward-data')

class Forward(upgrade.Upgrade):
    def __init__(self,config):
        upgrade.require(config.get('disposableApi') is True,'Future schemas require an explicitly disposable Kubernetes API')
        self.next={k:config[k] for k in ['nextInstaller','nextCatalog','nextChart']}
        super().__init__({k:v for k,v in config.items() if k not in self.next and k!='disposableApi'})
        self.baseline=dict(catalog=self.target,installer=self.installer,chart=Path(self.config['chart']),config=copy.deepcopy(self.config))
        future=json.loads(Path(self.next['nextCatalog']).read_text());upgrade.require(future['release']=='0.6.2-argocd-test.0' and future['apiSchemaRevision']==7,'Exact populated future schema fixture required')
        self.future=future
        self.report.update(test='forward-schema-cli-helm-rollback',source='0.6.1',target=future['release'],baseline='0.6.1',future=future['release'])
        self.report.pop('predecessor',None)
    def current_values(self):
        self.common()
        values=dict(installation=self.id,scope=self.config['scope'],runtimeProfile=self.config['runtimeProfile'],registryNode=self.primary,port=self.config['port'])
        if self.config['scope']=='namespace':values['applicationNamespace']=self.app
        else:values['excludedNamespaces']=self.excluded
        (self.output/'helm-values.json').write_text(json.dumps(values))
    def baseline_files(self):
        directory=self.output/'baseline-artifacts';directory.mkdir()
        for source,target in [(self.baseline['installer'],'kimage-v0.6.1-fixture'),(self.baseline['chart'],'kimage-0.6.1.tgz'),(Path(self.baseline['config']['catalog']),'component-images.json')]:shutil.copyfile(source,directory/target)
        self.report['baselineArtifactSha256']={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in directory.iterdir()}
        return directory
    def remove_consumers(self):
        # MixedCluster tracks every consumer, including Upgrade.consume().
        # Delete this single UID-bound inventory once, then await its Pods.
        for name,uid in reversed(self.api.deployments):
            current=self.api.get('deployment',name,self.app)
            if current is None:continue
            upgrade.require(current['metadata']['uid']==uid,'Fixture consumer identity changed')
            self.api.call(['delete','--raw','/apis/apps/v1/namespaces/'+self.app+'/deployments/'+name,'-f','-'],dict(apiVersion='v1',kind='DeleteOptions',propagationPolicy='Foreground',preconditions=dict(uid=uid)))
        self.api.wait(lambda:not self.api.get('pods',namespace=self.app,selector=upgrade.mixed.RUN_LABEL+'='+self.api.run_id)['items'],'all owned forward-reader consumers removed')
    def run(self):
        try:
            self.preflight();self.current_values();self.install_current()
            upgrade.mixed.ARCHITECTURES=self.architectures;self.api.preflight()
            self.build('baseline');self.verify_graphs('before-future-schema')
            self.previous=self.baseline_files();self.source='0.6.1';self.version=self.future['release'];self.target=self.future;self.installer=Path(self.next['nextInstaller'])
            self.config['catalog']=self.next['nextCatalog'];self.config['chart']=self.next['nextChart']
            self.transition(self.version,True)
            data=future_data.FutureData(self.api,self.system,self.architectures);data.write();self.report['forwardReader']=data.report
            self.verify_graphs('after-future-write')
            self.transition(self.source,True)
            # Use the unchanged baseline installer and component images now.
            self.version='0.6.1';self.target=self.baseline['catalog'];self.installer=self.baseline['installer'];self.config=self.baseline['config']
            data.after_return()
            self.consume(data.case['name'],self.architectures,unknown=True)
            self.installation_diagnostics();self.verify_graphs('after-baseline-reader-rollback')
            self.check('unchanged baseline readers retain schema 7 and opaque data while serving future roots and building unrelated Images')
            self.report['roundTrip']=['0.6.1',self.future['release'],'0.6.1']
            retained=self.retained()
            self.remove_consumers()
            self.uninstall_current();after=self.retained();upgrade.require(all(after[k]==retained[k] for k in ['pvcUid','pvUid','images','releases']),'Baseline uninstall changed retained future data')
            self.install_current();self.verify_graphs('after-baseline-reader-reinstall')
            self.uninstall_current();self.assert_removed();self.report['complete']=True
        finally:
            self.restore_labels()
            if self.private_config:Path(self.private_config.name).unlink(missing_ok=True)
            (self.output/'results.json').write_text(json.dumps(self.report,indent=2)+'\n')

def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--run',action='store_true');parser.add_argument('--config',type=Path,required=True);args=parser.parse_args()
    upgrade.require(args.run,'Opt in with --run before reading configuration or accessing a cluster')
    Forward(json.loads(args.config.read_text())).run()
if __name__=='__main__':main()

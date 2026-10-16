#!/usr/bin/env python3
"""Contract checks for the opt-in E2E; these never access Kubernetes."""
import copy
import importlib.util
import json
from pathlib import Path
import subprocess
import unittest

SCRIPT = Path(__file__).with_name('e2e-mixed-cluster.py')
spec = importlib.util.spec_from_file_location('mixed_e2e', SCRIPT)
mixed = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mixed)


def fixture(expected):
    def metadata(name,uid):
        return dict(name=name,uid=uid,generation=1)
    def status():
        return dict(observedGeneration=1,conditions=[dict(type='Ready',status='True')])
    image = dict(metadata=metadata('fresh','parent'),status=dict(status(),platformReleases={}))
    demand = dict(spec=dict(imageUid='parent',generation=1,platforms=[dict(os='linux',architecture=a) for a in sorted(expected)]))
    children, releases = [], []
    for arch in sorted(expected):
        child = dict(metadata=dict(metadata('child-'+arch,'uid-'+arch),labels={mixed.PARENT_LABEL:'parent'},ownerReferences=[dict(kind='Image',uid='parent')]),
            spec=dict(platform=dict(os='linux',architecture=arch)),status=dict(status(),releaseRef='child-release-'+arch))
        children.append(child)
        native = dict(metadata=metadata('native-'+arch,'release-'+arch),status=dict(verified=True,observedGeneration=1),spec=dict(imageUid='parent',
            platform=dict(os='linux',architecture=arch),root=dict(digest=arch,mediaType=mixed.MANIFEST),
            provenance=dict(execution=dict(platform=dict(os='linux',architecture=arch),executedSteps=1))))
        releases.append(native)
        child_release = copy.deepcopy(native)
        child_release['metadata']['name'] = 'child-release-'+arch
        child_release['spec']['imageUid'] = 'uid-'+arch
        releases.append(child_release)
        image['status']['platformReleases']['linux/'+arch] = 'native-'+arch
    if len(expected)==2:
        releases.append(dict(metadata=metadata('index','index-uid'),status=dict(verified=True,observedGeneration=1),
            spec=dict(imageUid='parent',platform=dict(os='linux',architecture='auto'),root=dict(digest='index',mediaType=mixed.INDEX))))
        image['status']['releaseRef'] = 'index'
    else:
        image['status']['releaseRef'] = 'native-'+next(iter(expected))
    return image,children,releases,demand,expected


class Contracts(unittest.TestCase):
    def test_disabled_test_has_no_cluster_access(self):
        result = subprocess.run(['python3',str(SCRIPT)],capture_output=True,text=True,check=True)
        self.assertIn('SKIP:',result.stdout)

    def test_platform_cases_do_not_predeclare_image_platforms(self):
        names = set()
        for arch in (None,'amd64','arm64'):
            name = 'fresh-'+str(arch)
            names.add(name)
            image = mixed.image_body(name,name+'-source')
            self.assertNotIn('platform',image['spec'])
            self.assertNotIn('platforms',image['spec'])
            self.assertIn('COPY case.txt',image['spec']['source']['build']['definition']['inline'])
            deployment = mixed.deployment_body(name,'apps',arch)
            pod = deployment['spec']['template']['spec']
            if arch:
                self.assertEqual(pod['nodeSelector'],{'kubernetes.io/arch':arch})
                self.assertEqual(deployment['spec']['replicas'],1)
            else:
                self.assertNotIn('nodeSelector',pod)
                self.assertNotIn('nodeName',pod)
                self.assertNotIn('nodeAffinity',pod['affinity'])
                self.assertEqual(deployment['spec']['replicas'],2)
        self.assertEqual(len(names),3)

    def test_valid_single_and_multi_platform_publication(self):
        for expected in ({'amd64'},{'arm64'},{'amd64','arm64'}):
            selected = mixed.assert_variants(*fixture(expected))
            self.assertEqual(selected['spec']['root']['mediaType'],mixed.INDEX if len(expected)==2 else mixed.MANIFEST)

    def test_false_positive_publications_are_rejected(self):
        def stale_demand(v): v[3]['spec']['imageUid']='old-image'
        def old_generation(v): v[3]['spec']['generation']=0
        def extra_child(v): v[1].append(copy.deepcopy(v[1][0]))
        def wrong_native_run(v): v[2][0]['spec']['provenance']['execution']['platform']['architecture']='arm64'
        def no_native_run(v): v[2][0]['spec']['provenance']['execution']['executedSteps']=0
        def foreign_child(v): v[1][0]['metadata']['ownerReferences'][0]['uid']='other'
        def old_release(v): v[2][0]['spec']['imageUid']='old'
        def unverified(v): v[2][0]['status']['verified']=False
        def foreign_child_release(v): v[2][1]['spec']['imageUid']='other'
        for mutate in (stale_demand,old_generation,extra_child,wrong_native_run,no_native_run,foreign_child,old_release,unverified,foreign_child_release):
            with self.subTest(mutation=mutate.__name__):
                value = list(fixture({'amd64'}))
                mutate(value)
                with self.assertRaises(RuntimeError):
                    mixed.assert_variants(*value)
        value = list(fixture({'amd64','arm64'}))
        value[2][-1]['spec']['root']['mediaType']=mixed.MANIFEST
        with self.assertRaises(RuntimeError):
            mixed.assert_variants(*value)


if __name__ == '__main__':
    unittest.main()

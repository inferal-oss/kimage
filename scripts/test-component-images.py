#!/usr/bin/env python3
"""Exercise native packaging and complete multi-platform registry publication graphs."""
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import struct
import tarfile
import tempfile
import unittest

SCRIPTS=Path(__file__).resolve().parent
def module(name):
    spec=importlib.util.spec_from_file_location(name,SCRIPTS/(name+'.py'))
    result=importlib.util.module_from_spec(spec);spec.loader.exec_module(result);return result
pack=module('package-component-images')
assemble=module('assemble-component-images')


def fixture(root,arch):
    root.mkdir()
    linux=root/'linux';linux.mkdir();buildkit=root/'buildkit';buildkit.mkdir()
    elf=bytearray(120);elf[:6]=b'\x7fELF\x02\x01'
    struct.pack_into('<H',elf,18,62 if arch=='amd64' else 183)
    struct.pack_into('<Q',elf,32,64);struct.pack_into('<HH',elf,54,56,1)
    for role in pack.ROLES:
        if role not in ['builder','registry']:(linux/('kimage-'+role)).write_bytes(elf)
    for name in ['buildkitd','buildctl','buildkit-runc']:(buildkit/name).write_bytes(elf)
    (root/'registry').write_bytes(elf)
    with contextlib.redirect_stdout(io.StringIO()):
        pack.package(linux,root/'registry',buildkit,root/'images',arch,'0.4.0')
    return root/'images'


class ImagesTests(unittest.TestCase):
    def test_complete_native_graphs_form_deterministic_digest_pinned_multiarch_catalog(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary)
            native=[fixture(root/arch,arch) for arch in ['amd64','arm64']]
            with contextlib.redirect_stdout(io.StringIO()):
                catalog=assemble.assemble(native,root/'assembled','ghcr.io/inferal-oss/kimage')
                again=assemble.assemble(list(reversed(native)),root/'again','ghcr.io/inferal-oss/kimage')
            self.assertEqual(catalog,again)
            self.assertEqual(set(catalog['components']),set(pack.ROLES))
            for role,component in catalog['components'].items():
                layout=root/'assembled'/role
                index=json.loads((layout/'blobs/sha256'/component['digest'][7:]).read_text())
                self.assertEqual([v['platform']['architecture'] for v in index['manifests']],['amd64','arm64'])
                self.assertEqual(component['platforms'],[dict(os='linux',architecture=arch) for arch in ['amd64','arm64']])
                for child in index['manifests']:
                    manifest=json.loads((layout/'blobs/sha256'/child['digest'][7:]).read_text())
                    self.assertEqual(manifest['layers'][0]['mediaType'],pack.oci.LAYER+'+gzip')
                self.assertEqual((root/'assembled'/(role+'.oci.tar')).read_bytes(),(root/'again'/(role+'.oci.tar')).read_bytes())
            self.assertFalse((root/'assembled'/'launcher').exists())
            self.assertEqual(catalog['components']['gateway']['version'],'0.3.0')
            self.assertEqual(catalog['components']['operator']['version'],'0.4.0')

    def test_packaged_chart_contains_exact_catalog_pins_and_is_deterministic(self):
        chart=module('package-chart')
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary);native=[fixture(root/arch,arch) for arch in ['amd64','arm64']]
            with contextlib.redirect_stdout(io.StringIO()):
                catalog=assemble.assemble(native,root/'images','ghcr.io/inferal-oss/kimage')
                first=chart.package(root/'images/component-images.json',root/'chart1')
                second=chart.package(root/'images/component-images.json',root/'chart2')
            self.assertEqual(first.read_bytes(),second.read_bytes())
            with tarfile.open(first,'r:gz') as tar:
                values=tar.extractfile('kimage/values.yaml').read().decode()
                for role,entry in catalog['components'].items():self.assertIn(role+': "'+entry['digest']+'"',values)
                self.assertIn('kimage/LICENSE',tar.getnames())
                self.assertFalse(any('crds/' in name for name in tar.getnames()))
            catalog['repository']='untrusted.invalid/mirror'
            (root/'images/component-images.json').write_text(json.dumps(catalog))
            with self.assertRaises(AssertionError):chart.package(root/'images/component-images.json',root/'bad')

    def test_missing_platform_or_wrong_native_executable_never_produces_catalog(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary);amd=fixture(root/'amd','amd64')
            with self.assertRaises(AssertionError):assemble.assemble([amd],root/'bad','ghcr.io/inferal-oss/kimage')
            self.assertFalse((root/'bad'/'component-images.json').exists())
            with self.assertRaises(ValueError):pack.package(root/'amd'/'linux',root/'amd'/'registry',root/'amd'/'buildkit',root/'wrong','arm64','0.4.0')
            self.assertFalse((root/'wrong'/'native-images.json').exists())

    def test_archive_checksums_do_not_replace_blob_integrity_checks(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary);amd=fixture(root/'amd','amd64');arm=fixture(root/'arm','arm64')
            archive=amd/'operator.oci.tar';members=[]
            with tarfile.open(archive,'r:') as t:
                for entry in t:
                    data=t.extractfile(entry).read()
                    if entry.name.startswith('blobs/') and not any(name=='corrupted' for name,_ in members):
                        data=data[:-1]+bytes([data[-1]^1]);members.append(('corrupted',b''))
                    members.append((entry.name,data))
            with tarfile.open(archive,'w') as t:
                for name,data in members:
                    if name!='corrupted':pack.oci.append(t,name,data)
            metadata=json.loads((amd/'native-images.json').read_text())
            metadata['components']['operator']['archive']=pack.oci.entry(archive)
            (amd/'native-images.json').write_text(json.dumps(metadata))
            with self.assertRaisesRegex(AssertionError,'corrupt OCI blob'):
                assemble.assemble([amd,arm],root/'bad','ghcr.io/inferal-oss/kimage')
            self.assertFalse((root/'bad'/'component-images.json').exists())

    def test_native_archive_cannot_reference_a_file_outside_its_directory(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary);amd=fixture(root/'amd','amd64');arm=fixture(root/'arm','arm64')
            metadata=json.loads((amd/'native-images.json').read_text())
            metadata['components']['admission']['archive']['file']='../../private'
            (amd/'native-images.json').write_text(json.dumps(metadata))
            with self.assertRaisesRegex(AssertionError,'invalid native archive path'):
                assemble.assemble([amd,arm],root/'bad','ghcr.io/inferal-oss/kimage')


if __name__=='__main__':unittest.main()

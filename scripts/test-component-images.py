#!/usr/bin/env python3
"""Exercise native packaging and complete multi-platform registry publication graphs."""
import contextlib
import argparse
import importlib.util
import io
import json
from pathlib import Path
import re
import struct
import tarfile
import tempfile
import unittest
from unittest.mock import patch

SCRIPTS=Path(__file__).resolve().parent
VERSION=re.search(r'(?ms)^\[workspace\.package\]\s*\n.*?^version\s*=\s*"([^"]+)"', (SCRIPTS.parent/'Cargo.toml').read_text())[1]
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
        pack.package(linux,root/'registry',buildkit,root/'images',arch,VERSION)
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
            self.assertEqual(catalog['components']['operator']['version'],VERSION)

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

    def test_release_assets_check_all_four_installer_architectures_and_exact_chart_pins(self):
        release=module('package-image-release');chart=module('package-chart')
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary);native=[fixture(root/arch,arch) for arch in ['amd64','arm64']]
            with contextlib.redirect_stdout(io.StringIO()):
                assemble.assemble(native,root/'images','ghcr.io/inferal-oss/kimage')
                chart_path=chart.package(root/'images/component-images.json',root/'chart')
            for system in ['linux','darwin']:
                for arch in ['amd64','arm64']:
                    directory=root/'installers'/(system+'-'+arch);directory.mkdir(parents=True)
                    if system=='linux':data=(root/arch/'registry').read_bytes()
                    else:
                        data=bytearray(16);data[:4]=bytes([207,250,237,254]);data[4:8]=(0x1000000+({'amd64':7,'arm64':12}[arch])).to_bytes(4,'little');data[12:]=bytes([2,0,0,0])
                    (directory/'kimage-install').write_bytes(data)
            metadata=root/'metadata.json';metadata.write_text(json.dumps(dict(packages=[])))
            args=argparse.Namespace(catalog=root/'images/component-images.json',chart=chart_path,installers=root/'installers',metadata=metadata,out=root/'release')
            original_read=Path.read_text
            def release_notes(path,*args,**kwargs):
                if path==SCRIPTS.parent/'CHANGELOG.md':return '# Changelog\n\n## ['+VERSION+'] - 2026-10-17\n\n### Added\n\n- Packaging fixture.\n'
                return original_read(path,*args,**kwargs)
            with patch.object(Path,'read_text',release_notes),contextlib.redirect_stdout(io.StringIO()):release.package(args)
            names={p.name for p in args.out.iterdir()}
            self.assertEqual(len(names),11) # Ten public assets plus release notes.
            self.assertIn('component-images.json',names);self.assertIn('release-compatibility.json',names);self.assertFalse(any('bundle' in n for n in names))
            for line in (args.out/'SHA256SUMS').read_text().splitlines():
                digest,name=line.split('  ');self.assertEqual(digest,pack.oci.digest((args.out/name).read_bytes())[7:])
            # A plausible ELF for the wrong ISA must not become an arm64 installer.
            (root/'installers/linux-arm64/kimage-install').write_bytes((root/'amd64/registry').read_bytes())
            args.out=root/'wrong'
            with contextlib.redirect_stdout(io.StringIO()),self.assertRaises(ValueError):release.package(args)
            self.assertFalse(args.out.exists())

    def test_missing_platform_or_wrong_native_executable_never_produces_catalog(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary);amd=fixture(root/'amd','amd64')
            with self.assertRaises(AssertionError):assemble.assemble([amd],root/'bad','ghcr.io/inferal-oss/kimage')
            self.assertFalse((root/'bad'/'component-images.json').exists())
            with self.assertRaises(ValueError):pack.package(root/'amd'/'linux',root/'amd'/'registry',root/'amd'/'buildkit',root/'wrong','arm64',VERSION)
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

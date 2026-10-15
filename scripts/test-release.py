#!/usr/bin/env python3
"""Regression tests for release packaging, pinned inputs, and immutable assets."""
import contextlib
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import struct
from subprocess import CompletedProcess
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch

SCRIPTS = Path(__file__).resolve().parent


def module(name):
    spec = importlib.util.spec_from_file_location(name, SCRIPTS / (name + '.py'))
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


bundle_pack = module('package-bundle')
release_pack = module('package-release')
inputs = module('fetch-release-inputs')
github = module('publish-github-release')
site_ready = module('site-release-ready')


def fixture(root, architecture="amd64", version="0.2.0"):
    linux, buildkit = root / 'linux', root / 'buildkit'
    linux.mkdir()
    buildkit.mkdir()
    elf = bytearray(120)
    elf[:6] = b'\x7fELF\x02\x01'
    struct.pack_into('<H', elf, 18, {'amd64':62,'arm64':183}[architecture])
    struct.pack_into('<Q', elf, 32, 64)
    struct.pack_into('<HH', elf, 54, 56, 1)
    for name in ('bootstrap', 'install', 'gateway', 'proxy', 'operator', 'admission', 'input', 'exporter'):
        (linux / ('kimage-' + name)).write_bytes(elf)
    for name in ('buildkitd', 'buildctl', 'buildkit-runc'):
        (buildkit / name).write_bytes(elf)
    (root / 'registry').write_bytes(elf)
    (root / 'macos').write_bytes(bytes([207, 250, 237, 254, 12, 0, 0, 1]) + b'\0' * 4 + bytes([2, 0, 0, 0]))
    target = root / 'bundle'
    arguments = ['package-bundle.py', '--linux-dir', str(linux), '--registry', str(root / 'registry'), '--buildkit-dir', str(buildkit), '--include-installers', '--macos-installer', str(root / 'macos'), '--component-version', version, '--architecture', architecture, '--serving-version', '0.2.0', '--out', str(target)]
    if version == '0.3.0':
        (root / 'macos-amd64').write_bytes(bytes([207,250,237,254,7,0,0,1]) + b'\0'*4 + bytes([2,0,0,0]))
        arguments.extend(['--macos-amd64-installer', str(root / 'macos-amd64')])
    with patch.object(sys, 'argv', arguments), contextlib.redirect_stdout(io.StringIO()):
        bundle_pack.main()
    return target


class ReleaseTests(unittest.TestCase):
    def test_both_native_release_bundles_and_four_installers_are_deterministic(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root/'LICENSE').write_text('license\n')
            (root/'CHANGELOG.md').write_text('# Changelog\n\n## [Unreleased]\n\n## [0.3.0] - 2026-10-15\n\n### Added\n\n- Native platforms.\n')
            for architecture in ['amd64','arm64']:
                location = root/architecture
                location.mkdir()
                bundle = fixture(location,architecture,'0.3.0')
                inventory = json.loads((bundle/'inventory.json').read_text())
                self.assertEqual(inventory['platform']['architecture'],architecture)
                self.assertEqual(inventory['apiSchemaRevision'],5)
                with patch.object(release_pack,'ROOT',root), contextlib.redirect_stdout(io.StringIO()):
                    release_pack.package(bundle,root/'release','0.3.0')
                wrong = 'arm64' if architecture == 'amd64' else 'amd64'
                with self.assertRaises(ValueError):
                    bundle_pack.static_elf(location/'linux/kimage-install',wrong)
                with tarfile.open(bundle/'input.oci.tar') as archive:
                    index=json.load(archive.extractfile('index.json'))
                    manifest=json.load(archive.extractfile('blobs/sha256/'+index['manifests'][0]['digest'][7:]))
                    config=json.load(archive.extractfile('blobs/sha256/'+manifest['config']['digest'][7:]))
                    self.assertEqual(config['architecture'],architecture)
            assets={path.name for path in (root/'release').iterdir() if path.name!='RELEASE_NOTES.md'}
            self.assertEqual(len(assets),11)
            for os in ['linux','darwin']:
                for arch in ['amd64','arm64']:
                    self.assertIn(f'kimage-v0.3.0-{os}-{arch}',assets)
            self.assertEqual(len((root/'release/SHA256SUMS').read_text().splitlines()),10)

    def test_site_waits_for_both_native_bundles_and_four_installers(self):
        version={'version':{'yanked':False}}
        names={f'kimage-bundle-v0.3.0-linux-{arch}{suffix}' for arch in ['amd64','arm64'] for suffix in ['.tar.gz','.inventory.json']}
        names.update(f'kimage-v0.3.0-{os}-{arch}' for os in ['linux','darwin'] for arch in ['amd64','arm64'])
        assets=[{'name':name,'state':'uploaded','digest':'sha256:fixture'} for name in names]
        for selected, expected in [(assets,True),(assets[:-1],False)]:
            with patch.object(site_ready,'get',side_effect=[version,version,{'draft':False,'assets':selected}]):
                self.assertEqual(site_ready.ready('0.3.0'),expected)

    def test_new_draft_visibility_delay_never_creates_another_draft(self):
        draft = {'id': 123, 'tag_name': 'v0.2.0', 'draft': True}
        with patch.object(github, 'release', side_effect=[None, None, draft]), patch.object(github.time, 'sleep') as sleep, patch.object(github.subprocess, 'run', side_effect=AssertionError('must not create another draft')):
            self.assertEqual(github.await_created_release('v0.2.0', 123), draft)
            self.assertEqual([call.args[0] for call in sleep.call_args_list], [1, 2])
        with patch.object(github, 'release', return_value=None), patch.object(github.time, 'sleep'):
            with self.assertRaisesRegex(RuntimeError, 'created draft 123'):
                github.await_created_release('v0.2.0', 123)
        with patch.object(github, 'release', return_value=draft):
            with self.assertRaisesRegex(AssertionError, 'lookup conflict'):
                github.await_created_release('v0.2.0', 456)

    def test_github_lookup_finds_drafts_and_resumes_without_duplicate_creation(self):
        missing = CompletedProcess([], 1, '', 'HTTP 404: Not Found')
        draft = {'tag_name': 'v0.2.0', 'draft': True, 'id': 123}
        page = CompletedProcess([], 0, json.dumps([draft]), '')
        with patch.object(github.subprocess, 'run', side_effect=[missing, page]) as run:
            self.assertEqual(github.release('v0.2.0'), draft)
            self.assertEqual(run.call_args.args[0][-1], 'repos/inferal-oss/kimage/releases?per_page=100&page=1')
        full = CompletedProcess([], 0, json.dumps([{'tag_name': f'v0.0.{i}'} for i in range(100)]), '')
        with patch.object(github.subprocess, 'run', side_effect=[missing, full, page]):
            self.assertEqual(github.release('v0.2.0'), draft)
        with patch.object(github.subprocess, 'run', side_effect=[missing, CompletedProcess([], 0, '[]', '')]):
            self.assertIsNone(github.release('v0.2.0'))
        with patch.object(github.subprocess, 'run', return_value=CompletedProcess([], 1, '', 'HTTP 403: Forbidden')):
            with self.assertRaises(RuntimeError):
                github.release('v0.2.0')

    def test_site_waits_for_both_crates_and_complete_public_bundle(self):
        version = {'version': {'yanked': False}}
        assets = [{'name': f'kimage-bundle-v0.2.0-linux-amd64{suffix}', 'state': 'uploaded', 'digest': 'sha256:fixture'} for suffix in ('.tar.gz', '.inventory.json')]
        release = {'draft': False, 'assets': assets}
        with patch.object(site_ready, 'get', side_effect=[version, None]):
            self.assertFalse(site_ready.ready('0.2.0'))
        with patch.object(site_ready, 'get', side_effect=[version, version, release]):
            self.assertTrue(site_ready.ready('0.2.0'))
        for incomplete in ({'draft': True, 'assets': assets}, {'draft': False, 'assets': assets[:1]}):
            with patch.object(site_ready, 'get', side_effect=[version, version, incomplete]):
                self.assertFalse(site_ready.ready('0.2.0'))

    def test_bundle_components_carry_real_release_and_separate_serving_versions(self):
        with tempfile.TemporaryDirectory() as temporary:
            target = fixture(Path(temporary))
            inventory = json.loads((target / 'inventory.json').read_text())
            self.assertEqual(inventory['components']['operator']['version'], '0.2.0')
            self.assertEqual(inventory['components']['gateway']['version'], '0.2.0')
            self.assertEqual(inventory['components']['registry']['version'], '3.1.2')
            self.assertEqual(inventory['components']['builder']['version'], '0.33.0')
            self.assertEqual(set(inventory['installers']), {'linux-amd64', 'darwin-arm64'})

    def test_release_assets_are_deterministic_and_preserve_inventory_and_payloads(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            bundle = fixture(root)
            (root / 'LICENSE').write_text('fixture license\n')
            (root / 'CHANGELOG.md').write_text('# Changelog\n\n## 0.2.0\n\nRelease notes.\n')
            with patch.object(release_pack, 'ROOT', root), contextlib.redirect_stdout(io.StringIO()):
                release_pack.package(bundle, root / 'first', '0.2.0')
                release_pack.package(bundle, root / 'second', '0.2.0')
            first, second = root / 'first', root / 'second'
            for file in first.iterdir():
                self.assertEqual(file.read_bytes(), (second / file.name).read_bytes())
            self.assertEqual((first / 'kimage-bundle-v0.2.0-linux-amd64.inventory.json').read_bytes(), (bundle / 'inventory.json').read_bytes())
            with tarfile.open(first / 'kimage-bundle-v0.2.0-linux-amd64.tar.gz') as archive:
                self.assertEqual(set(archive.getnames()), {file.name for file in bundle.iterdir()})
                for entry in archive.getmembers():
                    self.assertTrue(entry.isfile())
                    self.assertEqual(entry.mtime, 0)
                    self.assertEqual(archive.extractfile(entry).read(), (bundle / entry.name).read_bytes())
            for platform, installer in json.loads((bundle / 'inventory.json').read_text())['installers'].items():
                self.assertEqual((first / f'kimage-v0.2.0-{platform}').read_bytes(), (bundle / installer['file']).read_bytes())
            for line in (first / 'SHA256SUMS').read_text().splitlines():
                checksum, name = line.split('  ')
                self.assertEqual(checksum, hashlib.sha256((first / name).read_bytes()).hexdigest())

    def test_packaging_refuses_wrong_versions_unlisted_files_and_corrupt_components(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            target = fixture(root)
            with self.assertRaises(AssertionError):
                release_pack.files_for(target, '9.0.0')
            (target / 'unexpected').write_bytes(b'extra')
            with self.assertRaises(AssertionError):
                release_pack.files_for(target, '0.2.0')
            (target / 'unexpected').unlink()
            (target / 'kimage-input').write_bytes(b'corrupt')
            with self.assertRaises(AssertionError):
                release_pack.files_for(target, '0.2.0')

    def test_upstream_extracts_only_named_regular_files(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive = root / 'upstream.tar.gz'
            with tarfile.open(archive, 'w:gz') as tar:
                header = tarfile.TarInfo('../outside')
                header.size = 4
                tar.addfile(header, io.BytesIO(b'evil'))
                header = tarfile.TarInfo('bin/tool')
                header.size = 4
                tar.addfile(header, io.BytesIO(b'safe'))
            inputs.extract(archive, {'bin/tool': 'tool'}, root / 'out')
            self.assertEqual((root / 'out/tool').read_bytes(), b'safe')
            self.assertFalse((root / 'outside').exists())
            with tarfile.open(archive, 'w:gz') as tar:
                header = tarfile.TarInfo('bin/tool')
                header.type, header.linkname = tarfile.SYMTYPE, '../outside'
                tar.addfile(header)
            with self.assertRaises(AssertionError):
                inputs.extract(archive, {'bin/tool': 'tool'}, root / 'out')

    def test_upstream_cache_rechecks_size_and_content(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / 'archive'
            path.write_bytes(b'content')
            checksum = hashlib.sha256(b'content').hexdigest()
            self.assertTrue(inputs.verified(path, 7, checksum))
            path.write_bytes(b'changed')
            self.assertFalse(inputs.verified(path, 7, checksum))
            self.assertFalse(inputs.verified(path, 8, checksum))

    def test_github_asset_resume_refuses_conflicting_bytes(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / 'asset'
            path.write_bytes(b'expected')
            value = {'assets': [{'name': 'asset', 'size': 8, 'digest': github.sha256(path), 'state': 'uploaded'}]}
            github.verify_assets(value, [path])
            path.write_bytes(b'differen')
            with self.assertRaises(AssertionError):
                github.verify_assets(value, [path])


if __name__ == '__main__':
    unittest.main()

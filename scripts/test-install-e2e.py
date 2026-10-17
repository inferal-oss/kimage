#!/usr/bin/env python3
"""Regression checks for native install E2E preflight; no Kubernetes access."""
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('install_e2e', Path(__file__).with_name('e2e-install.py'))
install = importlib.util.module_from_spec(spec)
spec.loader.exec_module(install)


class Preflight(unittest.TestCase):
    def test_diagnostic_tail_accepts_independent_stdout_stderr_order_and_keeps_errors_private(self):
        assertions = install.log_assertions
        burst = ''.join(f'kimage-diagnostic-output-{line:04}\n' for line in range(5000))
        for output in ('kimage-diagnostic-failure-end\n' + burst, burst + 'kimage-diagnostic-failure-end\n'):
            tail = assertions.TRUNCATED + '\n' + output[-65536:]
            receipt = [dict(attempt='fixture', truncated=True, phase='Failed:ContainerfileSolveFailed')]
            assertions.verify(tail, tail, receipt, 'fixture')
        with self.assertRaisesRegex(RuntimeError, 'Live native failure output is missing') as error:
            assertions.verify('private-build-secret', tail, receipt, 'fixture')
        self.assertNotIn('private-build-secret', str(error.exception))
        self.assertIn('"finalLine": false', str(error.exception))
        receipt[0]['phase']='Failed:BuildProgressLineLimit'
        self.assertIn('Failed:BuildProgressLineLimit',assertions.summary('', '', receipt))
        receipt[0]['phase']='Failed: PRIVATE_ERROR'
        self.assertNotIn('PRIVATE_ERROR',assertions.summary('', '', receipt))
        receipt[0]['phase']='Failed:ContainerfileSolveFailed'
        with self.assertRaisesRegex(RuntimeError, 'Invalid bounded retained output'):
            assertions.verify(tail, burst, receipt, 'fixture')

    def test_public_diagnostic_context_only_contains_allowlisted_status_codes(self):
        summary=install.log_assertions.image_summary([dict(metadata=dict(name='private-name'),
            status=dict(operation=dict(phase='Executing'),conditions=[dict(reason='StaleOperation',message='private-message'),dict(reason='private-reason')]))])
        self.assertIn('StaleOperation',summary)
        self.assertIn('Executing',summary)
        self.assertNotIn('private',summary)

    def test_cache_reset_only_selects_owned_loopback_fixture_images(self):
        pause=dict(id='sha256:'+'1'*64,repoDigests=['docker.io/rancher/mirrored-pause@sha256:fixture'])
        role=dict(id='sha256:'+'2'*64,repoTags=None,repoDigests=['127.0.0.1:14901/kimage/input@sha256:fixture'])
        app=dict(id='sha256:'+'3'*64,repoTags=[],repoDigests=['127.0.0.1:17702/upgrade-app/image-uid@sha256:fixture'])
        self.assertEqual(install.fixture_cache_ids([pause,role,app,role]),[role['id'],app['id']])
        with self.assertRaisesRegex(AssertionError,'outside the owned CI fixture'):
            install.fixture_cache_ids([dict(id='sha256:'+'4'*64,repoTags=['127.0.0.1:18000/unrelated:latest'])])

    def test_incomplete_kubelet_reports_keep_waiting(self):
        for status in [{}, {'images': None}, {'images': []}, {'images': [{'names': None}]},
                       {'images': [{'names': ['unrelated:latest']}]}]:
            with self.subTest(status=status):
                self.assertIsNone(install.prerequisite_images({'status': status}))

    def test_ready_report_preserves_all_images_for_cold_cache_check(self):
        images = [{'names': None}, {'names': ['rancher/mirrored-pause@sha256:fixture']},
                  {'names': ['127.0.0.1:14901/kimage/input@sha256:unexpected-cache']}]
        self.assertEqual(install.prerequisite_images({'status': {'images': images}}), images)


if __name__ == '__main__':
    unittest.main()

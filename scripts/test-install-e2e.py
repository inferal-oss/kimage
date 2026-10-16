#!/usr/bin/env python3
"""Regression checks for native install E2E preflight; no Kubernetes access."""
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('install_e2e', Path(__file__).with_name('e2e-install.py'))
install = importlib.util.module_from_spec(spec)
spec.loader.exec_module(install)


class Preflight(unittest.TestCase):
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

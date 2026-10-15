#!/usr/bin/env python3
"""Require Keep a Changelog sections and notes for the current package version."""
from datetime import date
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]
text = (ROOT / 'CHANGELOG.md').read_text()
version = re.search(r'(?ms)^\[workspace\.package\]\s*\n.*?^version\s*=\s*"([^"]+)"', (ROOT / 'Cargo.toml').read_text())[1]
assert text.startswith('# Changelog\n') and 'keepachangelog.com/en/1.1.0/' in text
sections = re.findall(r'(?ms)^## \[([^\]]+)\]([^\n]*)\n(.*?)(?=^## |^\[Unreleased\]:|\Z)', text)
assert sections and sections[0][0] == 'Unreleased' and not sections[0][1].strip()
assert len({name for name, _, _ in sections}) == len(sections)
assert any(name == version for name, _, _ in sections), 'current release needs changelog notes'
for name, suffix, body in sections:
    if name == 'Unreleased':
        continue
    assert re.fullmatch(r' - \d{4}-\d{2}-\d{2}', suffix), 'released sections require ISO dates'
    date.fromisoformat(suffix.removeprefix(' - '))
    assert re.search(r'^### (Added|Changed|Deprecated|Removed|Fixed|Security)$', body, re.M)
    assert re.search(r'^- \S', body, re.M), 'released sections need concrete changes'
    assert re.search(r'^\['+re.escape(name)+r'\]: https://github.com/inferal-oss/kimage/', text, re.M)
assert re.search(r'^\[Unreleased\]: https://github.com/inferal-oss/kimage/compare/v'+re.escape(version)+r'\.\.\.HEAD$', text, re.M)
print('Keep a Changelog structure and current release notes verified.')

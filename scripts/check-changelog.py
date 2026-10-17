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
assert any(name == version for name, _, _ in sections) or any(name == 'Unreleased' and re.search(r'^- ',body,re.M) for name,_,body in sections), 'current or pending release needs changelog notes'
for name, suffix, body in sections:
    prose = re.sub(r'(?ms)^```.*?^```[^\n]*', '', body)
    for entry in re.findall(r'(?ms)^- .*?(?=\n(?:\n|- )|\Z)', prose):
        linked = re.search(r'\[[0-9a-f]{7,40}\]\(https://github.com/inferal-oss/kimage/commit/[0-9a-f]{40}\)', entry)
        pending = name == 'Unreleased' and 'Implementation pending commit.' in entry
        assert linked or pending, f'{name}: each change needs an inline commit link (or an explicit Unreleased pending marker)'
    if re.search(r'^- ', prose, re.M):
        assert re.search(r'^\*\*Commits:\*\* \[', body, re.M), f'{name}: missing inline commit list'
    if name == 'Unreleased':
        continue
    assert re.fullmatch(r' - \d{4}-\d{2}-\d{2}', suffix), 'released sections require ISO dates'
    date.fromisoformat(suffix.removeprefix(' - '))
    assert re.search(r'^### (Added|Changed|Deprecated|Removed|Fixed|Security)$', body, re.M)
    assert re.search(r'^- \S', body, re.M), 'released sections need concrete changes'
    assert re.search(r'^\['+re.escape(name)+r'\]: https://github.com/inferal-oss/kimage/', text, re.M)
latest=next(name for name,_,_ in sections if name!='Unreleased')
assert re.search(r'^\[Unreleased\]: https://github.com/inferal-oss/kimage/compare/v'+re.escape(latest)+r'\.\.\.HEAD$', text, re.M)
print('Keep a Changelog structure and current release notes verified.')

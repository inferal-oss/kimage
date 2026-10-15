#!/usr/bin/env python3
"""Check local Markdown links/anchors and parse the recorded JSON evidence."""
import json
import pathlib
import re
import sys
import urllib.parse

ROOT = pathlib.Path(__file__).resolve().parents[1]


def anchors(path):
    found = set()
    duplicates = {}
    in_code = False
    for line in path.read_text().splitlines():
        if line.startswith(('```', '~~~')):
            in_code = not in_code
        if in_code:
            continue
        heading = re.match(r'^#{1,6}\s+(.+?)\s*#*$', line)
        if heading:
            slug = re.sub(r'[^\w\- ]', '', heading[1].lower()).replace(' ', '-')
            count = duplicates.get(slug, 0)
            duplicates[slug] = count + 1
            found.add(slug if not count else f'{slug}-{count}')
    return found


errors = []
documents = [
    ROOT / 'README.md', ROOT / 'CHANGELOG.md', ROOT / 'site' / 'README.md',
    *sorted((ROOT / 'docs').rglob('*.md')),
    *sorted((ROOT / 'crates').glob('*/README.md')),
]
links = 0
for document in documents:
    for match in re.finditer(r'!?\[[^\]\n]*\]\(([^\s)]+)\)', document.read_text()):
        reference = urllib.parse.urlsplit(match[1])
        if reference.scheme or reference.netloc:
            continue
        links += 1
        destination = (
            (document.parent / urllib.parse.unquote(reference.path)).resolve()
            if reference.path else document
        )
        description = f'{document.relative_to(ROOT)}: {match[1]}'
        if not destination.is_relative_to(ROOT):
            errors.append(f'link leaves repository: {description}')
        elif not destination.exists():
            errors.append(f'missing destination: {description}')
        elif reference.fragment and destination.suffix == '.md':
            if urllib.parse.unquote(reference.fragment) not in anchors(destination):
                errors.append(f'missing heading: {description}')

receipts = sorted((ROOT / 'docs').rglob('*.json'))
for receipt in receipts:
    try:
        json.loads(receipt.read_text())
    except (ValueError, UnicodeError) as error:
        errors.append(f'{receipt.relative_to(ROOT)}: {error}')

if errors:
    print('\n'.join(errors), file=sys.stderr)
    sys.exit(1)
print(f'Checked {links} local links in {len(documents)} Markdown files and {len(receipts)} JSON receipts.')

#!/usr/bin/env python3
"""Validate the buildless website's local assets, anchors, and metadata."""
from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import unquote, urljoin, urlsplit
import xml.etree.ElementTree as ET
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1] / 'site'


class Page(HTMLParser):
    def __init__(self, canonical):
        super().__init__(convert_charrefs=True)
        self.expected_canonical = canonical
        self.ids = set()
        self.references = []
        self.controls = []
        self.headings = 0
        self.description = False
        self.canonical = False
        self.example = None
        self.examples = {}
        self.main_navigation = False
        self.changelog_navigation = False

    def handle_starttag(self, tag, attributes):
        attrs = dict(attributes)
        if tag == 'nav' and attrs.get('aria-label') == 'Main navigation':
            self.main_navigation = True
        if tag == 'a' and self.main_navigation:
            self.changelog_navigation |= urljoin(self.expected_canonical, attrs.get('href', '')) == 'https://kimage.dev/changelog/'
        if tag == 'pre' and 'data-example' in attrs:
            self.example = attrs['data-example']
            self.examples[self.example] = ''
        if 'id' in attrs:
            assert attrs['id'] not in self.ids, f'duplicate id: {attrs["id"]}'
            self.ids.add(attrs['id'])
        for attribute in ('src', 'href'):
            if attribute in attrs:
                self.references.append(attrs[attribute])
        for attribute in ('aria-controls', 'aria-labelledby'):
            self.controls.extend(attrs.get(attribute, '').split())
        if tag == 'img':
            assert 'alt' in attrs, 'image must have an alt attribute'
        if tag == 'h1':
            self.headings += 1
        if tag == 'meta' and attrs.get('name') == 'description':
            self.description = bool(attrs.get('content'))
        if tag == 'link' and attrs.get('rel') == 'canonical':
            self.canonical = attrs.get('href') == self.expected_canonical

    def handle_data(self, data):
        if self.example:
            self.examples[self.example] += data

    def handle_endtag(self, tag):
        if tag == 'nav':
            self.main_navigation = False
        if tag == 'pre':
            self.example = None


pages = {}
for file in ROOT.rglob('*.html'):
    relative = file.relative_to(ROOT).as_posix()
    canonical = 'https://kimage.dev/' + relative.removesuffix('index.html')
    page = Page(canonical)
    page.feed(file.read_text())
    assert page.headings == 1, f'{relative}: expected exactly one h1'
    assert page.description and page.canonical, f'{relative}: missing description or canonical URL'
    assert page.changelog_navigation, f'{relative}: missing changelog in main navigation'
    for control in page.controls:
        assert control in page.ids, f'{relative}: missing accessible target: {control}'
    for name, content in page.examples.items():
        assert content.strip() == (ROOT.parent / 'examples/hello' / name).read_text().strip(), f'handbook example drift: {name}'
    pages[file.resolve()] = page


def check_reference(source, reference):
    parsed = urlsplit(reference)
    if parsed.scheme or parsed.netloc:
        assert parsed.scheme == 'https', f'non-HTTPS external reference: {reference}'
        if parsed.netloc != 'kimage.dev':
            return
        path = ROOT / unquote(parsed.path).lstrip('/')
    else:
        path = source.parent / unquote(parsed.path) if parsed.path else source
    path = path.resolve()
    assert path.is_relative_to(ROOT), f'reference leaves site: {reference}'
    assert path.exists(), f'missing local asset: {reference}'
    if path.is_dir():
        path = path / 'index.html'
        assert path.exists(), f'missing directory index: {reference}'
    if parsed.fragment and path in pages:
        assert parsed.fragment in pages[path].ids, f'missing anchor: {reference}'


for file, page in pages.items():
    for reference in page.references:
        check_reference(file, reference)
for file in (ROOT / 'llms.txt', ROOT / 'docs/index.md'):
    for reference in re.findall(r'\]\(([^\s)]+)\)', file.read_text()):
        check_reference(file, reference)
assert (ROOT / 'llms.txt').read_text().startswith('# kimage\n\n> ')
version = re.search(r'(?ms)^\[workspace\.package\]\s*\n.*?^version\s*=\s*"([^"]+)"', (ROOT.parent/'Cargo.toml').read_text())[1]
assert f'cargo install kimage-control --version {version} --locked --bin kimage --bin kimage-install' in (ROOT/'site.js').read_text(), 'clipboard install command must match the documented release'
agent_image = re.search(r'```yaml\n(apiVersion:.*?)\n```', (ROOT / 'llms.txt').read_text(), re.S)
assert agent_image and agent_image[1].strip() == (ROOT.parent / 'examples/hello/image.yaml').read_text().strip(), 'agent Image example drift'
for name in ('main.rs', 'image.yaml', 'deployment.yaml'):
    assert (ROOT / 'examples/hello' / name).read_bytes() == (ROOT.parent / 'examples/hello' / name).read_bytes(), f'website example drift: {name}'
subprocess.run([sys.executable, str(ROOT.parent / 'scripts/export-site-docs.py'), '--check'], check=True)
subprocess.run([sys.executable, str(ROOT.parent / 'scripts/render-changelog.py'), '--check'], check=True)
ET.parse(ROOT / 'mark.svg')
sitemap = ET.parse(ROOT / 'sitemap.xml')
assert {entry.text for entry in sitemap.findall('.//{*}loc')} == {
    'https://kimage.dev/', 'https://kimage.dev/docs/', 'https://kimage.dev/changelog/'}
assert 'https://kimage.dev/sitemap.xml' in (ROOT / 'robots.txt').read_text()
print(f'Checked {len(pages)} pages, {sum(len(page.references) for page in pages.values())} HTML references, Markdown/agent links, example parity, accessible targets, SVG, and sitemap.')

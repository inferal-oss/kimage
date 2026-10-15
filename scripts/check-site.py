#!/usr/bin/env python3
"""Validate the buildless website's local assets, anchors, and metadata."""
from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import unquote, urlsplit
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[1] / 'site'


class Page(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.ids = set()
        self.references = []
        self.controls = []
        self.headings = 0
        self.description = False
        self.canonical = False

    def handle_starttag(self, tag, attributes):
        attrs = dict(attributes)
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
            self.canonical = attrs.get('href') == 'https://kimage.dev/'


page = Page()
page.feed((ROOT / 'index.html').read_text())
assert page.headings == 1, 'expected exactly one h1'
assert page.description and page.canonical, 'missing description or canonical URL'
for reference in page.references:
    parsed = urlsplit(reference)
    if parsed.scheme or parsed.netloc:
        assert parsed.scheme == 'https', f'non-HTTPS external reference: {reference}'
        continue
    path = (ROOT / unquote(parsed.path)).resolve() if parsed.path else ROOT / 'index.html'
    assert path.is_relative_to(ROOT), f'reference leaves site: {reference}'
    assert path.exists(), f'missing local asset: {reference}'
    if parsed.fragment:
        assert parsed.fragment in page.ids, f'missing anchor: {reference}'
for control in page.controls:
    assert control in page.ids, f'missing accessible control target: {control}'
ET.parse(ROOT / 'mark.svg')
sitemap = ET.parse(ROOT / 'sitemap.xml')
assert sitemap.find('.//{*}loc').text == 'https://kimage.dev/'
assert 'https://kimage.dev/sitemap.xml' in (ROOT / 'robots.txt').read_text()
print(f'Checked website metadata, {len(page.references)} references, accessible control targets, SVG, and sitemap.')

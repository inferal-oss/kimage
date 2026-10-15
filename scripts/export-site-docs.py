#!/usr/bin/env python3
"""Export the handbook's semantic HTML to Markdown without a build dependency."""
import argparse
from html.parser import HTMLParser
from pathlib import Path
import re
from urllib.parse import urljoin

ROOT = Path(__file__).resolve().parents[1]
BASE = 'https://kimage.dev/docs/'


class Node:
    def __init__(self, tag, attrs=()):
        self.tag = tag
        self.attrs = dict(attrs)
        self.children = []


class Article(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.root = None
        self.stack = []

    def handle_starttag(self, tag, attrs):
        node = Node(tag, attrs)
        if tag == 'article':
            assert self.root is None, 'expected one handbook article'
            self.root = node
        elif not self.stack:
            return
        else:
            self.stack[-1].children.append(node)
        if tag not in ('br', 'img', 'hr', 'input', 'meta', 'link'):
            self.stack.append(node)

    def handle_endtag(self, tag):
        if tag in ('br', 'img', 'hr', 'input', 'meta', 'link'):
            return
        if self.stack:
            assert self.stack[-1].tag == tag, f'unbalanced handbook tag: {tag}'
            self.stack.pop()

    def handle_data(self, data):
        if self.stack:
            self.stack[-1].children.append(data)


def plain(node):
    if isinstance(node, str):
        return node
    return ''.join(plain(child) for child in node.children)


def render(node):
    if isinstance(node, str):
        return re.sub(r'\s+', ' ', node)
    tag = node.tag
    if tag == 'div' and 'eyebrow' in node.attrs.get('class', '').split():
        return ''
    if tag == 'pre':
        language = 'yaml' if node.attrs.get('data-example', '').endswith('.yaml') else ''
        return '\n\n```' + language + '\n' + plain(node).strip('\n') + '\n```\n\n'
    if tag == 'table':
        rows = []
        for group in node.children:
            if isinstance(group, Node) and group.tag in ('thead', 'tbody'):
                for row in group.children:
                    if isinstance(row, Node) and row.tag == 'tr':
                        cells = [render(cell).strip().replace('|', '\\|')
                                 for cell in row.children if isinstance(cell, Node)]
                        rows.append('| ' + ' | '.join(cells) + ' |')
                        if group.tag == 'thead':
                            rows.append('| ' + ' | '.join(['---'] * len(cells)) + ' |')
        return '\n\n' + '\n'.join(rows) + '\n\n'
    body = ''.join(render(child) for child in node.children)
    if tag in ('h1', 'h2', 'h3'):
        return '\n\n' + '#' * int(tag[1]) + ' ' + body.strip() + '\n\n'
    if tag == 'code':
        return '`' + body.strip() + '`'
    if tag in ('strong', 'summary'):
        return '**' + body.strip() + '**' + ('\n\n' if tag == 'summary' else '')
    if tag == 'a':
        return '[' + body.strip() + '](' + urljoin(BASE, node.attrs['href']) + ')'
    if tag == 'br':
        return ' '
    if tag == 'li':
        return '\n- ' + body.strip()
    if tag in ('p', 'div', 'section', 'ul', 'ol', 'details'):
        return '\n\n' + body.strip() + '\n\n'
    return body


def export():
    parser = Article()
    parser.feed((ROOT / 'site/docs/index.html').read_text())
    assert parser.root is not None and not parser.stack
    markdown = render(parser.root).strip()
    # Remove indentation-only HTML whitespace without changing code indentation.
    markdown = re.sub(r'\n[ \t]+\n', '\n\n', markdown)
    markdown = re.sub(r'\n{3,}', '\n\n', markdown)
    markdown = '\n'.join(line.rstrip() for line in markdown.splitlines())
    return markdown + '\n'


if __name__ == '__main__':
    args = argparse.ArgumentParser(description=__doc__)
    args.add_argument('--check', action='store_true')
    options = args.parse_args()
    target = ROOT / 'site/docs/index.md'
    result = export()
    if options.check:
        assert target.read_text() == result, 'run python3 scripts/export-site-docs.py'
        print('Handbook Markdown matches its HTML source.')
    else:
        target.write_text(result)
        print('Exported site/docs/index.md from the handbook HTML.')

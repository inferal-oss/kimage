#!/usr/bin/env python3
"""Render the changelog to static HTML using only the Python standard library."""
import argparse
from datetime import date
from html import escape
from pathlib import Path
import re
from urllib.parse import urljoin

ROOT = Path(__file__).resolve().parents[1]
REPO = 'https://github.com/inferal-oss/kimage/'
TOKEN = re.compile(r'`([^`]+)`|\[([^\]]+)\]\(([^)]+)\)|\*\*([^*]+)\*\*')


def inline(value):
    """Escape prose first; only recognized Markdown can introduce HTML."""
    output = []
    start = 0
    for match in TOKEN.finditer(value):
        output.append(escape(value[start:match.start()]))
        code, label, href, strong = match.groups()
        if code is not None:
            output.append(f'<code>{escape(code)}</code>')
        elif label is not None:
            if not href.startswith('https://'):
                assert not re.match(r'[a-zA-Z][\w+.-]*:', href), f'unsupported link: {href}'
                href = urljoin(REPO + 'blob/main/', href)
            kind = ' class="commit-link"' if href.startswith(REPO + 'commit/') else ''
            output.append(f'<a{kind} href="{escape(href, quote=True)}">{escape(label)}</a>')
        else:
            output.append(f'<strong>{escape(strong)}</strong>')
        start = match.end()
    output.append(escape(value[start:]))
    return ''.join(output)


def blocks(body):
    lines = body.strip().splitlines()
    output = []
    index = 0
    while index < len(lines):
        line = lines[index]
        if not line:
            index += 1
            continue
        if line.startswith('```'):
            code = []
            index += 1
            while index < len(lines) and not lines[index].startswith('```'):
                code.append(lines[index])
                index += 1
            assert index < len(lines), 'unclosed code block'
            output.append('<pre><code>' + escape('\n'.join(code)) + '</code></pre>')
        elif line.startswith('### '):
            category = line[4:]
            assert category in ('Added', 'Changed', 'Deprecated', 'Removed', 'Fixed', 'Security')
            output.append(f'<h3 class="category category-{category.lower()}">{category}</h3>')
        elif line.startswith('- '):
            items = []
            while index < len(lines) and lines[index].startswith('- '):
                item = lines[index][2:]
                index += 1
                while index < len(lines) and lines[index].startswith('  '):
                    item += ' ' + lines[index].strip()
                    index += 1
                items.append('<li>' + inline(item) + '</li>')
            output.append('<ul>\n' + '\n'.join(items) + '\n</ul>')
            continue
        else:
            paragraph = [line]
            index += 1
            while index < len(lines) and lines[index] and not lines[index].startswith(('- ', '### ', '```')):
                paragraph.append(lines[index])
                index += 1
            text = ' '.join(paragraph)
            if text.startswith(('**Commits:**', '**Pull requests:**')):
                label, content = text.split(':**', 1)
                output.append(f'<details class="release-sources"><summary>{escape(label[2:])}</summary><p>{inline(content.strip())}</p></details>')
            else:
                output.append('<p>' + inline(text) + '</p>')
            continue
        index += 1
    return '\n'.join(output)


def render():
    markdown = (ROOT / 'CHANGELOG.md').read_text()
    references = dict(re.findall(r'^\[([^\]]+)\]: (https://\S+)$', markdown, re.M))
    markdown = markdown.split('\n[Unreleased]:', 1)[0]
    sections = re.findall(r'(?ms)^## \[([^\]]+)\]([^\n]*)\n(.*?)(?=^## |\Z)', markdown)
    navigation = []
    releases = []
    for index, (version, suffix, body) in enumerate(sections):
        pending = version == 'Unreleased'
        anchor = 'unreleased' if pending else 'v' + version
        label = 'Unreleased' if pending else 'v' + version
        navigation.append(f'<a href="#{anchor}"><span>{escape(label)}</span><span>{"Up next" if pending else escape(suffix.removeprefix(" - "))}</span></a>')
        if pending:
            stamp = '<span class="release-date">In development</span>'
            badge = '<span class="release-badge pending-badge">UP NEXT</span>'
        else:
            published = date.fromisoformat(suffix.removeprefix(' - '))
            stamp = f'<time class="release-date" datetime="{published.isoformat()}">{published.strftime("%B")} {published.day}, {published.year}</time>'
            badge = '<span class="release-badge">LATEST RELEASE</span>' if index == 1 else ''
        content = '<div class="release-body">' + blocks(body) + '</div>'
        if pending:
            count = len(re.findall(r'^- ', body, re.M))
            content = f'<details class="pending-changes"><summary>{count} upcoming changes</summary>{content}</details>'
        releases.append(f'''<article id="{anchor}" class="release{' unreleased' if pending else ''}">
<header class="release-heading"><div><h2>{escape(label)}</h2>{stamp}</div>{badge}</header>
{content}
<a class="release-compare" href="{escape(references[version], quote=True)}">{'View pending changes' if pending else 'View release changes'} <span aria-hidden="true">↗</span></a>
</article>''')
    template = (ROOT / 'scripts/templates/changelog.html').read_text()
    return template.replace('{{navigation}}', '\n'.join(navigation)).replace('{{releases}}', '\n'.join(releases))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true', help='reject stale generated HTML')
    args = parser.parse_args()
    target = ROOT / 'site/changelog/index.html'
    result = render()
    if args.check:
        assert target.exists() and target.read_text() == result, 'changelog page drift; run python3 scripts/render-changelog.py'
        print('Rendered changelog matches CHANGELOG.md and its template.')
    else:
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(result)
        print('Rendered site/changelog/index.html from CHANGELOG.md.')

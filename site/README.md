# kimage.dev website

Buildless HTML, CSS, JavaScript, and an original SVG mark. No package installation
or build step is needed. Edit `index.html`, `style.css`, and `site.js` directly.
The handbook lives in `docs/index.html` with its own `docs/docs.css`. It uses
ordinary anchors and native details panels. A small progressive enhancement
collapses the contents on mobile; the full page also works without JavaScript.
After editing its content, run `python3 scripts/export-site-docs.py` to regenerate
the clean Markdown download. The HTML is the source of truth; CI rejects drift.

`llms.txt` contains the essential agent context and a complete build example,
followed by a few optional references. Keep it useful without additional fetching.
The downloadable `examples/hello/` files must stay byte-identical to the repository
examples; static checks enforce this. Regenerate copies when editing the examples.

Preview from the repository root:

```sh
python3 -m http.server 8876 --bind 127.0.0.1 --directory site
```

Open `http://127.0.0.1:8876/`. Validate before publishing:

```sh
python3 scripts/check-site.py
node --check site/site.js
node --check site/docs/docs.js
```

The `Website` GitHub Actions workflow validates and uploads `site/`, then deploys
it to GitHub Pages. Pushes to `main` that change website files or its workflow
trigger validation; deployment waits until the workspace version has public crates
and official component catalog/chart assets. A successful release dispatches the website workflow
automatically. Manual dispatch is also available. Pull requests get the
static checks through the regular CI workflow without deploying.

GitHub Pages is configured for Actions deployment and the canonical domain
`kimage.dev`. Cloudflare DNS uses a DNS-only CNAME record for `@` pointing
to `inferal-oss.github.io`; Cloudflare flattens the apex CNAME. GitHub handles TLS.
Only the apex domain is configured. Custom-domain settings live in
GitHub Pages, not a CNAME file in this Actions artifact.

The browser QA covers desktop/mobile layout, tab selection with mouse and
keyboard, anchor navigation, and copying the Cargo install command. It also covers
the handbook's table of contents, details panels, Markdown download, and mobile
overflow. Manifest snippets
are labeled as excerpts; the complete hello example lives in `examples/hello/`.

Released Helm charts are fetched with verified GitHub asset SHA-256 values during
Pages packaging. `scripts/build-chart-repository.py` writes `charts/index.yaml`
and keeps every published chart version available under the apex domain.

# Repository instructions

## Changelog

Always keep `CHANGELOG.md` current using the
[Keep a Changelog 1.1.0](https://keepachangelog.com/en/1.1.0/) approach and format.

- Include a concise, user-facing changelog entry with every notable feature,
  behavior change, bug fix, deprecation, removal or security change. Update it
  in the same change as the implementation and relevant documentation.
- Keep `[Unreleased]` first. Put pending changes under the standard `Added`,
  `Changed`, `Deprecated`, `Removed`, `Fixed` or `Security` headings as appropriate.
  Omit empty category headings. Describe outcomes, not a dump of commit messages.
- When releasing, move the pending entries into `[X.Y.Z] - YYYY-MM-DD`, using
  Semantic Versioning and the publication date. Leave an empty Unreleased
  section for subsequent work.
- Maintain the comparison links at the bottom: Unreleased compares the latest
  release with HEAD, and each release compares with its predecessor.
- Preserve historical release notes and acceptance evidence. Correct factual
  mistakes explicitly; never relabel an old test receipt as a newer release.
- Run `python3 scripts/check-changelog.py` before committing and releasing.

## Release documentation

Follow `docs/releasing.md`. Keep package versions, the exact internal dependency,
Cargo.lock, chart metadata, README examples, website commands and `llms.txt` in
sync. Gateway/proxy serving compatibility has its own version. Published tags,
crate versions and release assets are immutable.

Use plain language in documentation. Do not use em dashes; use an en dash
surrounded by spaces only sparingly.

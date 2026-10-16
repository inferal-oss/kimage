# Releasing kimage

The public packages are `kimage-core` and `kimage-control`; the latter installs
eight commands. `kimage-bootstrap` is an unpublished internal bundle helper.
The unrelated `kimage` crate belongs to another project.

## Prepare the source

Update the workspace version and exact core dependency together, then update
Cargo.lock, crate READMEs, README, changelog, handbook, website commands, and
llms.txt. Keep the two crate licenses identical to the root Apache 2.0 license.
Gateway/proxy retain a separate serving compatibility version, currently 0.3.0;
a Cargo release does not implicitly change that binary compatibility contract.

```sh
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
python3 scripts/check-changelog.py
python3 scripts/test-release.py
python3 scripts/check-docs.py
python3 scripts/check-site.py
cargo publish --dry-run --locked --workspace --exclude kimage-bootstrap
```

Run the supported Linux/musl tests too. Commit and push the reviewed source to
main. The website validates the new documentation immediately but holds its
deployment until both crates and the complete GitHub release bundle are public.
This prevents the live quickstart from advertising an unavailable installation.

Maintain an empty `[Unreleased]` section above dated released sections, use the
Keep a Changelog categories (Added, Changed, Deprecated, Removed, Fixed, Security),
and update compare links. Record meaningful user behavior in the current release.
The checker runs in CI and the release validation job.

Native Kubernetes E2E is a publication gate: both installation scopes must pass on
native amd64 and arm64 runners. Review the E2E evidence artifacts before tagging.
The build preview also requires this reusable acceptance workflow.

## Build and review release assets

The pinned `publish.yml` workflow can preview a build from main without
publication or a publishing credential:

```sh
gh workflow run publish.yml --ref main -f dry_run=true
```

It validates source and docs, builds native Linux/amd64 and Linux/arm64 musl components and all four Linux/macOS
amd64/arm64 installers, downloads pinned upstream inputs, verifies
the full inventory/OCI graphs, and uploads `release-assets` to the Actions run.
The assets are:

- `kimage-bundle-v0.3.1-linux-<amd64|arm64>.tar.gz`, a complete flat offline bundle.
- `kimage-bundle-v0.3.1-linux-<amd64|arm64>.inventory.json`, also inside the bundle.
- Four installers: `kimage-v0.3.1-<linux|darwin>-<amd64|arm64>`.
- `LICENSE`, `THIRD-PARTY-NOTICES.txt`, and `SHA256SUMS`.

The input downloader checks pinned archive sizes/SHA-256 for Distribution 3.1.2
and BuildKit 0.33.0 before extracting only the named regular executables. Update
those pins deliberately with upstream verification, not by accepting a newly
adjacent checksum. The packer records the actual Cargo component version,
separately from registry/serving/builder versions. The release job checks real
binary version output and natively verifies the complete bundle before upload.

## Tag, verify OIDC, and publish

Create the version tag on the reviewed main commit, then dispatch a dry run:

```sh
git tag -a v0.3.1 -m 'kimage 0.3.1'
git push origin v0.3.1
gh workflow run publish.yml --ref v0.3.1 -f dry_run=true
```

Both crates trust only `inferal-oss/kimage`, workflow filename `publish.yml`, and
environment `crates-io`. That environment permits only tags matching `v*`. The
workflow additionally requires the tag to match the workspace version and its
commit to be on main. Only the tagged publishing job receives OIDC and release
write permissions. The official Rust authentication action exchanges GitHub's
identity for a temporary crates.io token and revokes it afterward. No long-lived
crates.io token is stored in GitHub secrets.

After inspecting the dry run, publish the same tag:

```sh
gh workflow run publish.yml --ref v0.3.1 -f dry_run=false
```

The job stages and hashes assets in a draft GitHub release, publishes core then
control through OIDC, and finalizes the GitHub release only after Cargo succeeds.
It runs the released Linux installer against the actual public GitHub download,
checks offline reuse and the independently generated inventory digest, then
dispatches the website deployment. Administrators can now fetch that exact bundle
without a checkout, Python script, kubeconfig, or GitHub token.

## Resume and verify

A repeated run preserves exact existing assets and never clobbers different
bytes. Crates are packaged and verified before checking publication state. An
already uploaded version is accepted only if its checksum matches the exact
candidate archive; a conflict stops the release. Partial publication can therefore
resume without replacing a crate or exposing an incomplete draft asset set.
Published release assets are treated as immutable. Do not move a published tag.

Verify the consumer path on macOS as well as the workflow's Linux check:

```sh
cargo install kimage-control --version 0.3.1 --locked --root .local/release-install
.local/release-install/bin/kimage bundle-fetch --bundle-cache .local/release-cache
.local/release-install/bin/kimage bundle-check --bundle-cache .local/release-cache --offline
```

Check all eight installed commands with `--help` and `--version`, the release and
site workflows, the live handbook/llms.txt, and a clean pushed main. For air gaps,
transfer the verified `bundle` directory and inventory digest through the chosen
trusted channel, then use `--bundle` and `--trusted-inventory sha256:...`.
Cluster/runtime/TLS/carrier prerequisites still apply. Automatic bundle delivery
does not create a general operator/schema upgrade or downgrade path.

Initial 0.1.0 publication used a local maintainer credential to create the two
crates before trusted publishers could be registered. That tag has no bundle
assets; retain its independently trusted local bundles/installers for recovery.

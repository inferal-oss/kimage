# Releasing kimage

The public packages are `kimage-core` and `kimage-control`. The latter installs
the seven native commands, including `kimage-install`. `kimage-bootstrap` is an
internal component-bundle helper and has publishing disabled. The unrelated
`kimage` package name on crates.io belongs to another project.

## Prepare

Update the workspace version and the exact `kimage-core` dependency in the control
manifest together. Update the install commands, crate READMEs, changelog, and site
for the intended version. Keep both crate copies of `LICENSE` identical to the
root Apache 2.0 license.

```sh
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo publish --dry-run --locked --workspace --exclude kimage-bootstrap
```

Also run Linux/musl tests using the prepared toolchain. Inspect `target/package/`
and confirm the archives contain sources, schemas, crate READMEs, and licenses,
with no local fixture state or credentials. The normal CI workflow verifies both
publishable packages by extracting and compiling them.

Gateway and proxy binaries carry a serving compatibility version, currently
`0.2.0`, independent of the initial `0.1.0` Cargo package release. Preserve that
version contract when changing command metadata.

## Publish and verify

Use an authenticated crates.io account authorized to publish these names. Keep
tokens out of source files, shell arguments, logs, and chat. Configure Cargo with
`cargo login` interactively or a suitable credential provider, then publish from
the clean release commit:

```sh
cargo publish --locked --workspace --exclude kimage-bootstrap
```

Cargo publishes the dependency before the control package. If publication is
interrupted, check which exact versions are already present before resuming with
only the remaining package.

Verify installation from crates.io into an isolated prefix, then check all seven
commands with `--help` and `--version`:

```sh
cargo install kimage-control --version 0.1.0 --locked \
  --root .local/release-install
.local/release-install/bin/kimage-install --version
```

Push the release commit and tag `v0.1.0`, publish the release notes, and confirm
the site deployment. Installing the CLI does not provision a cluster or supply
an offline component bundle. Those remain explicit installation prerequisites.

## Trusted publishing

Both crates trust only GitHub repository `inferal-oss/kimage`, workflow filename
`publish.yml`, and environment `crates-io`. The GitHub environment permits only
tags matching `v*`. No crates.io API token is stored in GitHub secrets.

The workflow is manually dispatched on the intended release tag. It verifies
that the tag matches the workspace version and its commit is on `main`, runs the
source checks, then exchanges GitHub's OIDC identity for a short-lived publishing
token using the pinned official Rust action. The action revokes it after the job.
Use the default dry run first:

```sh
gh workflow run publish.yml --ref v0.1.0 -f dry_run=true
```

For a new, unpublished version, dispatch the matching tag with `dry_run=false`
after review. This publishes both packages in dependency order. An already
published version is immutable; do not attempt to republish it. Initial 0.1.0
publication uses the local maintainer credential because the crates must exist
before their trusted publisher can be registered. Inspect the first tagged OIDC
dry run to verify federation without uploading the same release again.

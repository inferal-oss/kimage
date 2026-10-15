# Changelog

## 0.2.0 (2026-10-15)

- Official GitHub Actions builds now produce a complete Linux/amd64 component
  bundle, its inventory and checksums, and native Linux/amd64 and macOS/arm64
  installers as GitHub release assets. Administrators no longer need to package
  their own bundle for the supported profile.
- `kimage-install` automatically fetches the exact official release matching its
  package version, verifies GitHub asset digests, inventory, static executables,
  and complete OCI graphs, and caches it before Kubernetes access.
- `bundle-fetch` prepares an installation or air-gap transfer without kubeconfig;
  `--release`, `--bundle-cache`, and `--offline` make selection and reuse explicit.
  Local `--bundle` with an independently trusted inventory remains supported.
- Atomic cache installation, bounded archive extraction, process locking, and
  regression coverage for corrupt, interrupted, unsafe, and incomplete downloads.
- Release publishing stages verified draft assets, publishes crates through OIDC
  with safe partial-release resume, then publishes GitHub assets and verifies the
  real public fetch and offline path.
- Updated handbook, README, quickstart, and self-contained agent guidance.

```sh
cargo install kimage-control --version 0.2.0 --locked
kimage-install bundle-fetch
kimage-install bundle-check --offline
```

The supported Kubernetes/runtime, persistent storage, cached carrier, issuer
credentials, and user namespace prerequisites still apply. Automatic fetching
changes bundle delivery; it does not broaden the serving-only upgrade window or
provide an operator/schema downgrade path. See the
[handbook](https://kimage.dev/docs/).

## 0.1.0 (2026-10-15)

Initial alpha release for the documented Linux/amd64 Kubernetes runtime profile.

- Native `kimage-install` command for API-only bootstrap, verification, recovery,
  and compatible serving upgrades and rollback.
- Declarative Image production through pinned BuildKit, verified OCI publication,
  and retained immutable ImageRelease resources.
- Exact, channel, and readable colon/query range selectors with digest-pinned Pod
  admission and ordinary kubelet pulls through node-local proxies.
- Separate retained-registry storage and disposable worker cache, two-node
  delivery/recovery, and an optional independent registry exporter.
- Operational metrics, offline bundle tooling, documented live acceptance
  evidence, and macOS/Linux test coverage.

Install the native commands with Rust 1.99.0 or newer:

```sh
cargo install kimage-control --version 0.1.0 --locked
kimage-install --help
```

Cluster installation requires a trusted Linux/amd64 component bundle, TLS
credentials, and a supported runtime profile. Builds initially support scratch
images and provisioned local inputs. See the
[compatibility limits](README.md#compatibility-and-limits).

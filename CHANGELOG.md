# Changelog

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

# Changelog

All notable changes to this project are documented here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.1] - 2026-10-15

### Fixed

- Removed the 30-second socket read timeout from the Kubernetes client. Healthy
  quiet watches and long-lived exec/port-forward streams must not expire at that
  limit. A regression test covers a delayed watch event; native installation E2E
  checks that a fresh Image is reconciled after a quiet catalog interval.

### Added

- An explicitly enabled mixed-cluster E2E using fresh Images for unknown, amd64,
  and arm64 workload placement. It checks native builds, exact variant and demand
  sets, and native execution through the selected OCI index or manifest. Public
  CI checks the harness; the live test requires an existing mixed cluster.
  [Two recorded real-cluster runs](docs/mixed-cluster-results.json) passed using
  the published 0.3.0 binaries.

### Changed

- Clarified README and website positioning against kpack: both provide
  declarative Images; kimage integrates an included registry, immutable version
  catalog, and workload image/version requests resolved during Pod admission.

## [0.3.0] - 2026-10-15

### Added

- Native Linux/amd64 and Linux/arm64 workers, architecture-specific bundles, and
  Linux/macOS installers for both architectures. Installation selects bundles
  from cluster Node architecture and registers native workers automatically.
- Placement-aware image selection from node names, architecture selectors and
  required node affinity. Unconstrained mixed-cluster workloads receive verified
  multi-platform OCI indexes; every child graph and layer is checked.
- Independent immutable Image UID/version/platform releases and platform status.
  Aggregate indexes require consistent input captures across native variants.
- Default on-demand production for automatic Images, bounded generation-scoped
  ImageDemand records, and an administrator ConfigMap for optional eager builds.
  Dry-run requests produce no demand; explicit platforms request direct builds.
- Namespace-scoped and cluster-wide installation, runtime scope filters,
  administrator exclusions, overlap checks and same-namespace image consumption.
- Native Kubernetes E2E for both scopes on amd64 and arm64, required before release
  publication. Coverage includes real builds/pulls, controller templates, build
  policy, dry-run, RBAC, namespace boundaries and retained storage on uninstall.
- Keep a Changelog structure and CI validation for future release notes.

### Changed

- Omitted Image platform now selects native platforms automatically. Explicit
  `platform` and `platforms` constrain native variants. Apply a consumer workload
  before waiting for an automatic Image under the default OnDemand policy.
- Schema revision 5 accepts exact shipped revision 2/3/4 definitions, preserving
  data and rejecting drift or downgrades. Serving binaries report version 0.3.0;
  cluster scope requires that version's namespace routing support.
- Interrupted registry publication retries within a bounded deadline using fresh
  upload sessions and complete byte readback. Integrity and authorization errors
  still fail immediately.
- Uninstall tolerates status-only resource-version races while retaining UID and
  ownership checks and refusing concurrent spec edits or replacement resources.
- README, tutorial, handbook and agent essentials document platform inputs,
  selection, demand/eager policy, both scopes and automatic bundle delivery.

## [0.2.0] - 2026-10-15

### Added

- Official GitHub Actions builds now produce a complete Linux/amd64 component
  bundle, its inventory and checksums, and native Linux/amd64 and macOS/arm64
  installers as GitHub release assets. Administrators no longer need to package
  their own bundle for the supported profile.
- `kimage install` runs the standard setup phases in one resumable command. It
  validates all credential directories before bootstrap writes and avoids
  repeating bootstrap after a partial handover. `kimage-install` remains a
  compatible alias with the same commands.
- The installer automatically fetches the exact official release matching its
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
kimage install --help
```

The supported Kubernetes/runtime, persistent storage, cached carrier, issuer
credentials, and user namespace prerequisites still apply. Automatic fetching
changes bundle delivery; it does not broaden the serving-only upgrade window or
provide an operator/schema downgrade path. See the
[handbook](https://kimage.dev/docs/).

## [0.1.0] - 2026-10-15

### Added

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

[Unreleased]: https://github.com/inferal-oss/kimage/compare/v0.3.1...HEAD
[0.3.1]: https://github.com/inferal-oss/kimage/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/inferal-oss/kimage/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/inferal-oss/kimage/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/inferal-oss/kimage/releases/tag/v0.1.0

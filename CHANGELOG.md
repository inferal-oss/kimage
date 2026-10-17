# Changelog

All notable changes to this project are documented here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.1] - 2026-10-16

### Added

- Argo CD chart mode with deterministic offline rendering, an in-cluster provisioner, automatic native node discovery and generated or externally supplied credentials. Applications expose readiness through the provisioner Deployment and support both installation scopes.
- Argo CD ownership, guarded runtime transitions and explicit removal that preserves registry data, Images, releases and shared CRDs. Existing CLI/Helm installations are not adopted automatically; Argo CD ownership requires 0.5.1 or newer.
- Argo CD chart-version upgrades and compatible downgrades from 0.5.1 onward. A returning older chart automatically delegates to the installed newer operator by digest, preserving the lifecycle lock and resumable journal, without manual catalog selection.
- Copyable Argo CD Applications and installation, configuration, health and retention guidance in the README, handbook and agent essentials.

### Changed

- Verified the published Cargo CLI and public Helm repository through real Argo CD with the chart's built-in pins, native amd64/arm64 builds and retained data after removal/reinstall. The [published-artifact receipt](https://github.com/inferal-oss/kimage/blob/main/docs/published-0.5.1-results.json) also checks automatic catalog fetching, public asset checksums and anonymous component access.
- Recorded [native acceptance](https://github.com/inferal-oss/kimage/blob/main/docs/native-0.5.1-results.json) and [real mixed-cluster acceptance](https://github.com/inferal-oss/kimage/blob/main/docs/mixed-0.5.1-results.json), including both Argo CD scopes, interrupted chart-version upgrades/downgrades, native placement and retained-data readback. Earlier attempts remain recorded alongside the complete repeat.
- CLI and Helm transition acceptance now targets published 0.5.0 to 0.5.1 and back. Published 0.5.0 resource templates are frozen for rollback; historical acceptance evidence remains unchanged.

### Fixed

- Helm upgrades from 0.5.0 tolerate reused values without an Argo CD configuration section.
- Release instructions use the supported adjacent 0.5.0 to 0.5.1 upgrade and reverse transition. Argo removal guidance and acceptance wait for provisioner Pods to finish before runtime cleanup.
- Argo CD transitions verify retained image bytes through the authenticated in-cluster registry service, preserving TLS, root digest and size checks without API port-forward privileges.
- Argo CD runtime uninstall waits for downgrade helper Jobs and Pods to disappear, preventing them from racing explicit removal.
- Installation and GitOps policy updates recheck installation identity and pending operations after acquiring the lifecycle lock, refusing stale preflight state before applying resources.

## [0.5.0] - 2026-10-16

### Added

- Explicit installation upgrade and compatible rollback commands, with read-only checks, an exclusive Lease, resumable phases and retained storage/credential identity verification. The initial supported transition is 0.4.1 to 0.5.0 and back, without storage or API migration.
- Automatic node membership and native worker reconciliation, with UID-bound serving approvals, administrator node selection and worker pins, and fencing before replacement.
- Read-only `kimage status` and `kimage doctor`, including JSON output, pending operations, component readiness, native platforms, retained storage, certificate validity and resource drift.
- Private build diagnostics through `kimage logs IMAGE --follow` and `kimage builds IMAGE`, with platform and attempt selection. Completed attempt logs survive build Pod cleanup; each platform retains the latest five attempts with a bounded 64 KiB output tail per attempt. Guides explain access permissions, retry policy, output clipping and progress limits.

### Changed

- Recorded [eight native lifecycle cases](docs/native-upgrade-results.json) and [real mixed-cluster CLI/Helm round trips](docs/mixed-upgrade-results.json) through actual published 0.4.1, including interruption/resume, native pulls/builds, node eligibility/worker changes, private failure logs and retained-image recovery after uninstall/reinstall.

- After rollback to the published 0.4.1 controller, the documented Helm removal procedure stops its operator and retires its recreated workers before chart removal. Acceptance guards worker identity/specification, recognizes Kubernetes' omitted false defaults, and refuses foreign Kubernetes or Helm owners. Fresh 0.5.0 Helm removal is tested directly.

### Fixed

- Guarded Helm transitions coordinate proxy and worker replacement with the predecessor operator, recover interrupted history without immutable Job conflicts, and tolerate transient restart backoff with Helm 4's legacy readiness wait. Ordinary three-way updates preserve Helm ownership and revision history.
- CLI rollback resume stops partially restarted predecessor components again. Concurrent worker creation is accepted only for the exact owned, non-deleting specification. Uninstall rechecks pending operations and installation identity under its Lease and stops on Lease loss.
- Node reconciliation repairs missing ownership on unchanged resources without restarting them or taking over another owner. Worker recovery and operation journals retain installation ownership for chart adoption and cleanup. Required Pod/Job patch access is limited to the operator's system namespace; doctor identifies denied permissions safely.
- Existing immutable credential views are verified without rewriting their metadata or requesting additional Secret write permissions. New views receive installation ownership at creation.
- Resource checks compare JSON values across Helm and Rust serialization, preserving manager identity inside worker recovery templates while rejecting unexpected fields and genuine drift.
- Native readiness Jobs track node identity, placement and input images, and fit Kubernetes label limits for every supported installation name. Readiness checks wait through normal worker replacement without accepting ownership or specification drift.
- Build-log snapshots pair terminal phases with final output. Following an attempt waits through startup and bounded transient connection failures, reports gaps, and retains full attempt identities in valid diagnostic Secret names.
- Builds wait for a replacement worker's Pod and service endpoints before capturing inputs, avoiding empty interrupted attempts during normal worker recovery.
- The BuildKit progress adapter accepts batched records up to 1 MiB, preventing valid output bursts from failing at the old 256 KiB record limit. Total progress stays bounded to 16 MiB and retained text to 64 KiB.
- Candidate acceptance repeats failed-build diagnostics with fresh native Images to catch progress batching and recovery races before image publication.
- Upgrade direction follows semantic version ordering, and native node lifecycle features remain enabled in later releases.
- Native diagnostic acceptance tolerates independent stdout/stderr ordering. Cleanup progress and failures expose only bounded metadata; focused lifecycle diagnostics cover both architectures and installation scopes.

### Security

- Optional candidate and focused native workflows encrypt failing Helm output to a maintainer-provided certificate; the private decryption key remains outside CI. Focused runs can reuse unchanged native component artifacts and run without capture. Native attempt summaries expose only allowlisted phase and public status codes and bounded metadata.
- Diagnostic Secrets carry the predecessor's reserved input marker, so rolling back to 0.4.1 cannot make private build output eligible as an ordinary build input.
- Helm upgrade checks reject recovery history that would replay stale credential or build-policy bytes, before stopping components.

## [0.4.1] - 2026-10-16

### Added

- Recorded [real mixed-cluster acceptance](docs/mixed-install-results.json) for the published 0.4.0 CLI and public Helm chart: native GHCR pulls, fresh unknown/amd64/arm64 placement cases, retained resource identities and complete OCI graph readback after uninstall/reinstall.
- Repository instructions now require Keep a Changelog entries and synchronized release documentation for future changes.

### Changed

- Updated Cargo and Helm installation examples, README, website and agent essentials for 0.4.1, which includes the CLI uninstall fix.

### Fixed

- CLI uninstall retries status-only resource-version conflicts when Kubernetes LIST items omit the type metadata included by GET. UID, specification and ownership checks remain enforced. This fixes an intermittent `CleanupIdentityConflict` observed while testing published 0.4.0 on a real mixed cluster.
- Native installation E2E waits through transient null kubelet image reports instead of failing before installation. Regression coverage keeps the cold-cache check intact.

## [0.4.0] - 2026-10-16

### Added

- Native amd64/arm64 component images published to GHCR by the release workflow, with complete immutable OCI indexes and public-pull verification.
- A Helm chart repository at `https://kimage.dev/charts`, containing the same release image pins and resource definitions as the CLI.
- Eight native Kubernetes acceptance cases covering CLI and Helm, namespace and cluster scope, on both architectures, using fresh TLS registry pulls.

### Changed

- `kimage install` now discovers nodes, generates credentials and applies ordinary Kubernetes resources. It fetches only a small verified image catalog; component executables and layers no longer travel through the Kubernetes API.
- Installation defaults to the current kubeconfig application namespace, a dedicated system namespace and the default StorageClass. Reapplication preserves credentials, build policy and registry node identity.
- Releases contain four installers, the image catalog and pinned chart instead of bootstrap bundles. Legacy bootstrap commands remain available for independently retained older installations.
- CLI and Helm uninstall remove disposable installation resources while retaining the registry PVC, Images, ImageReleases and shared CRDs.
- Updated installation guides, quickstarts, website and agent essentials for image-pull and Helm installation.

### Fixed

- Fresh installs use an explicit serving ServiceAccount, avoiding dependence on asynchronous creation of the namespace's default account.
- Helm upgrades preserve administrator-owned build policy without taking ownership of its fields. Reapplication preserves the original policy JSON bytes.

## [0.3.1] - 2026-10-15

### Fixed

- Removed the 30-second socket read timeout from the Kubernetes client. Healthy
  quiet watches and long-lived exec/port-forward streams must not expire at that
  limit. A regression test covers a delayed watch event; native installation E2E
  checks that a fresh Image is reconciled after a quiet catalog interval.
- Release bundle verification now checks gateway/proxy against their separate
  serving version, allowing patch releases to retain serving compatibility.

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

[Unreleased]: https://github.com/inferal-oss/kimage/compare/v0.5.1...HEAD
[0.5.1]: https://github.com/inferal-oss/kimage/compare/v0.5.0...v0.5.1
[0.5.0]: https://github.com/inferal-oss/kimage/compare/v0.4.1...v0.5.0
[0.4.1]: https://github.com/inferal-oss/kimage/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/inferal-oss/kimage/compare/v0.3.1...v0.4.0
[0.3.1]: https://github.com/inferal-oss/kimage/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/inferal-oss/kimage/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/inferal-oss/kimage/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/inferal-oss/kimage/releases/tag/v0.1.0

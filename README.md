# kimage

Rust implementation of a Kubernetes image production and delivery extension.

Workloads can select a published image using a readable version range:

```yaml
containers:
  - name: service
    image: "platform://apps/service:^1.2.0"
```

Exact versions (`:1.2.4`), channels (`:stable`) and query ranges
(`?version=^1.2.0`) are also supported. Admission pins each new Pod to a verified
image digest. See the [selector reference](docs/operations.md) for selection and
rollout behavior.

Start with the [implementation brief](docs/implementation-prompt.md) and
[verified PoC findings](docs/poc-findings.md). Experimental source and full evidence
are preserved on the separate orphan branch `codex/poc-spikes`.

The implementation has a native API-only installer, maintained PVC-backed
Distribution registry, separate authenticated reader/publisher identities, a
loopback read proxy, OCI graph verification/publication/recovery, typed
Image/ImageRelease declarations, guarded schema startup and read-only admission
primitives. Normal kubelet pulls deliver images.

See the [architecture](docs/architecture.md), [operations guide](docs/operations.md)
and [acceptance ledger](docs/status.md). The isolated fixture verifies native
operator/admission provisioning, archive-to-platform-Pod delivery, continuous
manifest reads during handover, cache-absent Linux-native recovery, credential
rotation and sandboxed Direct builds. All four acceptance milestones in the
original brief passed for the documented supported profile. Real binary upgrade
and rollback cover Distribution 3.1.1 ↔ 3.1.2 and gateway/proxy 0.1.0 ↔ 0.2.0,
including interrupted rollout and cache-absent older-bundle recovery. See the
[foundation](docs/milestone-1-acceptance.md),
[build](docs/milestone-2-acceptance.md),
[two-node delivery/recovery](docs/milestone-3-acceptance.md) and
[optional export](docs/milestone-4-acceptance.md) acceptance reports. The offline
bundle includes the default BuildKit worker, optional independent exporter and
native macOS/Linux installers. Read the reports for the supported scratch/local
build, serving-image upgrade and HTTPS authentication windows; this is not an HA
or arbitrary storage/binary migration claim. The [requirements audit](docs/requirements-audit.md)
maps the original requirements to evidence and explicit limits. The final bundle
is `dist/complete-final-linux-amd64`; its runtime receipts precede the readable
colon-range addition. The source passes 49 tests on macOS and Linux/musl.

```sh
cargo test --locked --offline --workspace
cargo fmt --all --check
cargo clippy --locked --offline --workspace --all-targets -- -D warnings
```

Licensed under [Apache License 2.0](LICENSE).

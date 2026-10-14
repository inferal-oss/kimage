# Architecture PoC findings — 2026-10-14

Experimental code and full evidence are preserved on the orphan branch
`codex/poc-spikes` (root commit `58da0332555e689975b9f9b5560a049db8536845`). Inspect without changing branches:

```sh
git show codex/poc-spikes:docs/status.md
git show codex/poc-spikes:docs/feasibility-evidence.md
git show codex/poc-spikes:docs/image-path-spike.md
git show codex/poc-spikes:docs/build-path-spike.md
git show codex/poc-spikes:docs/build-path-results.json
```

`main` is a fresh implementation baseline. No experimental Rust code was merged.
The source prompt and prototypes remain accessible in Git; ignored local build
outputs/credentials are not part of that snapshot and must not be committed.

## What actually passed

| Area | Evidence and limits |
|---|---|
| API-only bootstrap | Native macOS installer transferred bounded executable chunks through ConfigMaps/API; cached carrier ran static launcher, Distribution registry on PVC and read proxy. No kimage OCI image was externally pulled; no SSH/runtime socket/direct import/node edits. |
| Runtime endpoint | Actual containerd/kubelet digest pulls through loopback read-only HTTP proxy, authenticated verified-TLS upstream. Application root requests captured in proxy logs. Host network proxy used existing namespace permissions. |
| Recovery/handover | Registry/proxy all stopped, API-only recover reused the retained PVC; subsequent fresh content pulled and ran. Stop/start handover had downtime. Ordinary component images were not evicted from the runtime cache. |
| Archive source | Isolated credential-free reader froze an OCI archive from a same-namespace source PVC; verifier checked graph/config/platform/DiffIDs; publisher uploaded original bytes and read them back. |
| Release/admission | Image → immutable verified ImageRelease → actual HTTPS API-server webhook → platform:// Pod → normal IfNotPresent digest pull → Running. Exact/channel/range, dry-run, UID binding, node constraint, missing version, forged metadata, version conflict and release immutability checks. Scope was bypassable opt-in experiment routing. |
| Direct builds | Pinned BuildKit v0.33.0, builtin Dockerfile frontend v1.27.0, OCI/runc native-snapshot worker. Inline and nested ConfigMap-mounted definitions; binary executable/text/key/mode mapping; separate definition/context roots; actual RUN context bind mount and persisted output. |
| Cache/input changes | Disabled and Local solve modes exercised. New input bytes at version 1.0.1 produced new input hashes/roots for both definition forms; 1.0.0 release UID/spec stayed unchanged. No full cache matrix or reproducible-build claim. |
| Retention | Inputs and BuildKit worker/cache deleted, operator restarted, cache mode changed; both retained 1.0.1 releases still resolved and ran. This proves published artifact independence, not in-flight solve restart recovery. |
| Build negatives | Changed declaration for same version → VersionConflict; missing mounted definition → BuildFailed with no release. Path/symlink/mode and export-root verification unit coverage. |
| Checks | Final spike passed formatting, warnings-denied clippy and 18 unit tests on macOS; 16 core/engine library tests also passed on Linux/amd64 musl. Ten real build-path Pods reached Running, in addition to earlier archive/runtime tests. |

No full original milestone passed. Complete acceptance gaps are in the archived
ledger and [revised implementation prompt](implementation-prompt.md).

## Reuse the decisions, replace the shortcuts

- Registry is authoritative. Kubernetes metadata is not an image store; runtime
  caches and BuildKit caches are disposable. Preserve OCI graph identity.
- Dedicated input capture and supported BuildKit session transport work. A file
  mounted into a reader is not implicitly present on a remote worker or inside RUN.
- Mounted definition location must not alter the ordinary build context root.
- The built-in frontend can be pinned with BuildKit; external syntax overrides
  were refused. RUN network was none; FROM scratch avoided external source pulls.
- Direct uses one dedicated worker, not a per-build execution Job. The worker was
  privileged but had no hostPath/runtime socket/API credential; its unauthenticated
  API was loopback-only and reached through authenticated Kubernetes forwarding.
- Immutable ConfigMaps made the captured fixture stable. General mutable-object
  resource-version snapshots, Secret sessions/definitions and PVC/subPath build
  fixtures were not implemented. A read-only PVC mount does not quiesce a writer.
- Pinned buildctl was a trusted transport client, invoked without a shell and
  with explicit paths/arguments. Build execution stayed outside the controller.
- Opt-in labels/HMAC metadata are experiment isolation, not tenant authorization.
  Shared backend credentials, temporary certificates and manual package steps are
  not a complete installation/credential lifecycle.
- Source/request hashes are not provenance, reproducibility or secret-safe
  attestations. Actual worker attempt identity still needs a durable model.

## Tested environment and retained state

- Kubernetes `v1.36.5+rke2r1`, containerd `2.3.4-k3s1`, Linux/amd64 node
  `x86-ex130r-1`, namespace `dev-yrashk`, `local-path` RWO storage.
- Rust `1.99.0`; Distribution `3.1.2`; BuildKit `v0.33.0` (release tar SHA256
  `b6242896d343100808dcbe37565caf381e0a444a6a83d7255926bb1519248ead`).
- Registry PVC `kimage-native-20261014-data`, UID
  `ac63d957-55dd-414b-b2b0-0756720d0f69`. It holds authoritative PoC
  data and must be preserved. Temporary registry/workloads/webhooks/RBAC removed.
- Retained Images: `kimage-image-path-20261014-app`,
  `kimage-build-path-20261014-inline`, `kimage-build-path-20261014-mounted`.
  Two archive releases plus four build releases remain verified. Both CRDs and
  built-in schema Lease/checkpoint also remain. Reinventory before mutation.
- Cached approved bootstrap carrier on the tested node:
  `docker.io/alpine/k8s@sha256:b7a12c5ddf261994c33d2eaaa06fd69a0803ff6b38683bfa3d30a76dcdf92807`.
  Its current availability is a preflight check, not a permanent guarantee.
- Use existing `$HOME/.kube/inferal-config`, context `inferal`; environment helpers
  are in `/Users/yrashk/Projects/inferaldata/inferal-workspace/scripts/env.bash`
  (Bash). Do not edit that infrastructure repository or print credentials.
- API hostname `k8s.inferal.io` had mixed reachability: `88.99.93.100` and
  `188.34.142.208` worked; `5.9.16.19` timed out externally. The test harness used
  `--server=https://88.99.93.100:6443 --tls-server-name=k8s.inferal.io`, preserving
  CA/hostname verification without editing kubeconfig/DNS. Recheck this temporary
  observation. Restricted sandbox DNS failed; supported elevated networking worked.

The final sanitized build receipts are also copied to
[PoC build results](poc-build-results.json). Full historical details remain on the
archive branch. `git switch codex/poc-spikes` restores the experimental checkout
when no other task is actively using this working directory.

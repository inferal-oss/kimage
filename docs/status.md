# Clean implementation acceptance ledger

2026-10-17, 0.6.0 candidate: [eight native CLI/Helm and four Argo CD cases](native-0.6.0-results.json)
pass upgrades from actual published 0.5.2, interruptions, schema downgrade refusal,
frozen OCI inputs and retained-data reinstall. [Real mixed-cluster acceptance](mixed-0.6.0-results.json)
upgrades the permanent cluster-wide Argo installation and tests fresh unknown,
amd64 and arm64 consumers, warm/disabled cache, denied sources, a missing native
base platform and later arm64 demand after same-name base UID replacement.
All material roots remain frozen. Installation/storage identities, all nine
service credentials, administrator policy and 203 retained release contracts
remain intact. Complete readback verifies 18 prior and 35 new release graphs.
Status/doctor and default discovery pass. These checks use immutable public
candidate images before the final tag; publication is verified separately.
Shared Nodes are unchanged and the permanent runtime remains installed.

2026-10-17, 0.5.2: [eight native CLI/Helm cases and four Argo CD cases](native-0.5.2-results.json)
pass predecessor transitions, interruptions and retained-image recovery.
The [published-artifact and real-upgrade receipt](published-0.5.2-results.json)
checks exact public bytes, Cargo installation and the live chart, then upgrades
a permanent cluster-wide Argo installation from 0.5.1 on four real mixed nodes.
Fresh placement cases pass before and after the upgrade. Registry, installation,
credential and release identities remain unchanged; complete retained graphs
remain readable. Default discovery and doctor succeed. This permanent runtime
is left installed; native fixtures cover interruption, rollback and uninstall.

2026-10-16, 0.5.1: [native acceptance](native-0.5.1-results.json) and
[real mixed-cluster acceptance](mixed-0.5.1-results.json) cover CLI/Helm
published 0.5.0 → 0.5.1 → 0.5.0, plus actual Argo CD in both scopes.
Argo changes chart versions through a separate future-version conformance
fixture and back, recovering interrupted provisioners and newer downgrade helpers.
Mixed checks include fresh placement cases, native GHCR pulls, reversible node
changes, retained identities and complete OCI graph readback through removal and
reinstallation. Earlier unsuccessful attempts and the full repeat remain recorded;
historical release receipts below are unchanged.

2026-10-16, 0.5.0: [eight native cases](native-upgrade-results.json) and
[real mixed-cluster CLI and Helm cases](mixed-upgrade-results.json) passed the
actual published 0.4.1 → 0.5.0 → 0.4.1 round trip. Evidence covers interrupted
upgrade/rollback and resume, native pulls/builds, status/doctor, retained private
failed-attempt diagnostics, retained identities and full OCI graph readback after
rollback and uninstall/reinstall. Mixed cases also cover reversible node eligibility,
new native workers and worker pin replacement, with selection restored before
rollback. No shared Node deletion/cordon or registry storage migration occurs.
Earlier receipts and their recorded limits below remain unchanged.

For 0.3.0 platform selection, default on-demand production, eager configuration,
and namespace/cluster installation, see [native platforms and scope](platforms-and-scope.md).
Earlier acceptance receipts below describe the historical amd64 fixture; native
Kubernetes E2E now covers both architectures and both installation scopes.

2026-10-15. **All four original acceptance milestones passed for the documented
supported profile.** The binary-upgrade gap is closed by real Distribution
3.1.1 ↔ 3.1.2 and gateway/proxy 0.1.0 ↔ 0.2.0 cycles, including interrupted
resume, newer writes read after older-binary rollback, cache-absent older-bundle
recovery and Linux-native roll-forward. Live metrics and post-upgrade retained
export regressions also passed. See `requirements-audit.md` for the complete
requirement-to-evidence mapping and explicit unsupported extensions.
Readable colon ranges such as `platform://apps/service:^1.2.0` are supported in
the source alongside literal and encoded query ranges. Parser equivalence,
catalog selection/no-match behavior and normal/init-container admission patches
are covered by the current 49-test suites; the earlier live bundle receipts
remain unchanged and predate this addition.
See `docs/milestone-1-acceptance.md` and `docs/milestone-2-acceptance.md` for evidence
and limits, plus `docs/milestone-3-acceptance.md` and
`docs/milestone-4-acceptance.md` for two-node recovery/upgrades and exports.
No required acceptance gate is left unverified within that supported profile.
The experimental branch remains separate. Only the standalone OCI verifier and
URI parser were selectively reused and reviewed; the control plane, resource model,
gateway, installation journal and lifecycle harness were implemented on `main`.

## Implemented foundation

- Rust workspace with locked dependencies and native installer/operator/admission/
  input/gateway/proxy/exporter binaries plus a static Linux launcher.
- OCI image manifests and nested single-platform indexes, original compressed or
  uncompressed layer bytes, config/platform/DiffID checks, bounded disk spooling,
  SHA-256 validation and topological publication with independent registry readback.
  Multi-platform images, zstd layers, descriptor URLs/embedded data and subject/
  referrer graphs are unsupported. Verified graph metadata has read-only getters.
- Retained graph download/verification without the original input archive. Registry
  transport has deadlines, cancellation and origin checks; errors omit upload URLs.
- Distribution 3.1.2 behind a TLS gateway. Backend binds Pod loopback; readers are
  separately authenticated and cannot write. Node proxy is loopback-only,
  digest-only, GET/HEAD-only, with verified HTTPS upstream and no redirects/fallback.
- Native bounded ConfigMap executable transfer, trusted-inventory validation,
  static ELF validation, resource ownership checks, durable installation phases,
  warmup pulls, single-writer ordinary registry/proxy handover, a durable handover
  checkpoint, API recovery and uninstall that preserves PVCs/custom resources/CRDs.
- Typed archive Image/ImageRelease model, status CAS, durable operation identities,
  immutable releases, content idempotence, generation acknowledgment before sealing
  and detached artifact retention. Source archive capture uses a credential-free
  non-root reader and a binary API stream. Its root init helper only prepares the
  disposable snapshot volume; it never mounts or changes the source PVC.
- Operator schema phase before typed watches, optimistic Lease ownership, definition/
  revision/field-ownership guards, establishment/discovery checks, non-crashing
  pending-schema health and refusal of incompatible transitions.
- Strict URI grammar, exact/channel/range/prerelease selection, coherent per-Image
  catalog capture, retained-UID fallback, signed request-bound resolution metadata,
  JSON Patch, init sidecars, affinity conjunction and unsupported update rejection.
  Admission performs no publication and no Kubernetes writes.

## Earlier foundation interoperability evidence

All live resources used the unique `kimage-foundation-fdedd0a9` installation in
`dev-yrashk`, on the explicitly approved amd64 test node. Registry data used a new
1 GiB RWO PVC; the PoC registry PVC was never mounted or changed.

| Check | Result and limit |
| --- | --- |
| Native macOS API-only bootstrap | Passed: cached Never carrier, projected static launcher, bounded executable chunks, new PVC, registry/TLS gateway and hostNetwork loopback proxy. No helper OCI image, runtime socket, node edits or external kimage pull. |
| Component seeding | Six component OCI images published and independently read back through authenticated native Kubernetes forwarding. The development bundle lacks the default builder and packaged native installers. |
| Credential separation | Actual reader GET 200, reader upload POST 403, publisher POST 202 and owned-upload DELETE 204. Reader credentials never enter the publisher identity. |
| TLS | Native Rust validation refused an incorrect CA-as-server test certificate. Separate CA/server leaf fixed it; the running gateway reloaded the projected leaf. TLS verification stayed enabled. This is not a complete renewal/CA-rotation controller. |
| Fresh normal pulls | Three unpredictable new probe roots reached Running with IfNotPresent, with exact root requests captured by the proxy: after bootstrap, after ordinary handover and after API recovery. This exercises direct digest references; it is not a new Image → ImageRelease → API-server platform admission proof. |
| Ordinary handover | All six component images warmed with normal pulls; ordinary registry/gateway/proxy served from digest images. Single-writer stop/start has downtime. Idempotent repeat passed. |
| Interrupted handover | Installer terminated after the durable replacement checkpoint and registry deletion. Resume observed the registry absent, recreated ordinary serving from the checkpoint and verified retained content. No simultaneous registry writers. |
| All fixture serving processes stopped/recovered | Ordinary uninstall left the same PVC; cached-carrier API recovery restored retained graph readback and a fresh normal pull. Component images remained in node cache, so cache-absent recovery acceptance is still open. |
| Missing carrier | Refused before any resources were created; no fallback pull was attempted. Namespace/admission denial coverage remains open. |
| Actual API schema checks | Both generated definitions passed server validation. A unique temporary API group established both CRDs, accepted a valid declaration and rejected mixed sources, invalid prerelease, parent traversal and immutable release mutation. Both fixture CRDs/objects were removed. This is structural API evidence, not native manager startup/upgrade acceptance. |
| Non-root input capture | Actual Linux binary in a disconnected, read-only, capabilities-dropped container captured and streamed identical archive bytes with a separate writable snapshot. Symlink escape refused. Kubernetes PVC/exec capture through the new operator is still unverified. |

Fresh probe roots:

```text
sha256:715fff6bd5ba929a718ded317a0a9b9b8984831d1712d0716a6734ea8a8bcbaa
sha256:3484001cac9037bae5d57e8e180b0e86589bd43e12ad4a74e2e589834f60b831
sha256:7784981fe17ae7d94c285fee1d7dfdfd1588f32f868603f664a112142d4dedd7
```

The earlier shared-cluster run used Kubernetes `v1.36.5+rke2r1`, containerd
`2.3.4-k3s1`, Linux/amd64, Distribution `3.1.2`, Rust `1.99.0`, kube `4.2.0`,
k8s-openapi `0.28.0`/`v1_36`, reqwest `0.12.28`, semver `1.0.28`.
Ordinary CRI image/runtime services and the node's existing snapshotter were used;
no kimage runtime plugin was installed. BuildKit `v0.33.0`/frontend `v1.27.0`
were still PoC pins at that earlier checkpoint. The current complete bundle ships
the default builder and is tested in the separate owned two-node K3s fixture.

## Milestone acceptance (current)

| Milestone | Current result |
| --- | --- |
| 1 | Passed for the explicitly pinned single-node baseline; evidence and failure limits are in `milestone-1-acceptance.md`. This does not claim multi-node support, self-upgrade, declarative builds, or HA. |
| 2 | Passed for the pinned scratch/local-input Direct baseline: complete mounted/secret/cache matrix, immutable input/version handling, fencing, restart and retained-artifact recovery. See `milestone-2-acceptance.md` and its receipts. |
| 3 | Passed: two-node delivery/recovery, actual binary-version upgrade/interrupted resume/reverse rollback, newer-data readback after older-binary rollback, all-eight-component cache-absent older-bundle recovery, Linux-native roll-forward, additive schemas and compatible operator coexistence. See `milestone-3-acceptance.md` and `binary-upgrade-results.json`. |
| 4 | Passed: independent retained-artifact export, authenticated separate registry, exact remote graph/tag verification, idempotence/conflicts, interruption/retry, credential rotation, external-downtime independence and removal without remote deletion. Final-binary initial-status and retained-export checks also passed. See `milestone-4-acceptance.md` and `export-design.md` for the HTTPS Basic/anonymous profile and limits. |
| Cross-cutting final checks | Passed: fixed-cardinality build/publication/pull latency and transfer counters, queue/readiness/freshness gauges, real HTTP completion/truncation regression, actual fresh build/pull aggregate metrics, post-upgrade retained export with publisher absent, and final exact-image readiness on both nodes. |

At the milestone acceptance checkpoint, the shared cluster retained revision-1
experimental CRDs. The current operator refuses to adopt or overwrite those
unsupported definitions. Those native acceptance runs used a separate owned QEMU
cluster and left shared Inferal resources and node configuration untouched.
Earlier evidence tables below/above are historical and retain their original limits.
The later [opt-in mixed-cluster test](mixed-cluster-e2e.md) uses its separately
approved fixture and requires a compatible current installation.
Two independent runs with official 0.3.0 binaries passed on real RKE2 amd64/arm64
nodes. The [mixed-platform receipt](mixed-cluster-results.json) records six fresh
parent Image identities, exact native variant sets, selected roots, and native
consumer platforms. This is separate from the required homogeneous native CI
matrix and does not broaden the older lifecycle evidence.

## Reproducible checks

```sh
cargo fmt --all --check
cargo clippy --locked --offline --workspace --all-targets -- -D warnings
cargo test --locked --offline --workspace
cargo build --locked --offline --workspace --bins
```

The current implementation passes 49 meaningful tests on macOS and
Linux/amd64 musl (47 library tests plus two API recovery regressions). The earlier
runtime bundle's 47-test receipt remains in `final-bundle-results.json`. Strict
workspace/all-target Clippy passed on macOS; the offline Linux toolchain lacks
Clippy, while its full compile/tests passed. Formatting and Python syntax checks
pass. Exact platform checks and the final inventory are in `final-bundle-results.json`.

See `scripts/live-foundation.py`, `scripts/fault-handover.py` and
`scripts/package-bundle.py`. Scripts take explicit options; tests enforce the
authorized context/namespace and unique ownership prefix. Native APIs support an
optional explicit server address plus TLS server name without kubeconfig edits.
Linux binaries were built with musl in a task-owned Rust container disconnected
from networking after toolchain preparation. Connected preparation downloaded
musl tools/target; neither that nor Pod NetworkPolicy proves a disconnected runtime.

Earlier shared-fixture cleanup left its new labelled registry data PVC. The original
PoC registry PVC UID, three Images and six releases were preserved. Its two temporary
schema fixture CRDs were removed without changing retained product definitions.
Current isolated acceptance preserves its own registry PVC and release/schema
identities; `final-health-results.json` records both nodes and all seven final
serving/control/exporter Pods Ready on the exact final bundle image digests.

Local credential files, source archives, compiled binaries and bundle outputs stay
in ignored `.local/` and `dist/`. Sanitized results are separate from secret-bearing
logs/configuration. Ordinary uninstall preserves CRDs/releases/PVCs; cleanup only
removes the fixture's disposable serving resources.

## Historical implementation log

The following entries preserve incremental milestone-1 checkpoints. Their
unfinished statements and older test counts are superseded by the current
acceptance table and the four milestone reports above.

Work resumed after the foundation checkpoint; the ordered four-milestone brief
remains in scope. The foundation was not a completion boundary.

The native installer now has a `control-plane` phase with separate system and
application namespaces. It provisions ordinary digest-image operator/admission
Pods, separate ServiceAccounts and narrowly scoped roles, publisher-only operator
credentials, an independent admission certificate/key and resolution secret, and
an application-namespace webhook. Activation waits for operator readiness,
compatible established CRDs and independently verified admission TLS readiness.
Uninstall removes only owned serving/RBAC/webhook resources; schemas and schema
coordination remain retained. The actual installation and archive-to-Pod path passed in the isolated fixture.

Admission grants the fixed input helper an exception only for the API-authenticated
operator ServiceAccount. Author labels/annotations cannot grant it. Managed
resolution additionally checks the selected node's UID/Ready/platform and the
owned proxy's upstream-backed readiness. External references do not depend on
these managed-image eligibility checks. Generation monitoring cancels obsolete
operations; durable failure conditions distinguish permanent validation/content
conflicts from retries with bounded exponential delay. Old-generation input Pods
and completed helpers use ownership and deletion preconditions.

A full amd64 QEMU fixture uses Kubernetes `v1.36.5+k3s1`, containerd
`2.3.4-k3s1.36`, Ubuntu kernel `6.8.0-142-generic`, prerequisite-only K3s airgap
images, a restricted user network and a loopback-forwarded API. It successfully
started DNS/storage workloads without a kimage image. Authenticated native binary
OCI publication/readback and six-component seeding succeeded through API
forwarding. The final fixture uses IPv4 Pod/service networking and passed the native
controller/admission/managed-Pod path; IPv6 runtime support remains unclaimed.

The fixture exposed a cached-carrier USER assumption: bootstrap now selects an
explicit UID/GID instead of inheriting image USER. Service listeners explicitly
accept both IPv4 API forwarding and IPv6 Pod/service traffic; the registry proxy
continues to bind only IPv4 loopback. The new dual-stack socket test initially
could not bind in the filesystem/network sandbox, then passed through the supported
permission route. There are now 21 passing macOS library tests. The subsequent macOS and static Linux test run passed 23 library tests.

Earlier fixture attempts failed: amd64 kind under Docker emulation could not boot,
and the task-owned OrbStack Linux machine rejected Kubernetes `tmpfs noswap`
mounts. Those fixture environments were removed. No shared node configuration,
runtime cache, retained PoC CRD or PVC was modified for them. Fixture sources are
`scripts/isolated-fixture.py`, `scripts/fixtures/qemu/`, and the actual-controller
acceptance driver `scripts/live-controller.py`; prerequisites are separate from
product bootstrap and do not contain kimage OCI images.

### Complete archive/controller/admission proof and cold recovery

The routed full-VM fixture passed the new PVC archive → native operator → verified
ImageRelease → API-server admission → normal kubelet path. The producer terminated
before capture. An unpredictable root absent from the initial CRI image inventory
started regular and restartable init-sidecar containers with IfNotPresent; the
proxy recorded its exact manifest request. Unmanaged containers were preserved,
server dry-run created no Pod or release update, and raw internal/cross-namespace
references were denied. Source PVC and Image removal did not remove the release;
a second exact-selector Pod ran from the retained graph.

Root: `sha256:5906842771e9c2ac5abfcec58731833c4590b8f1920cbfe609c99c8f8e177fcc`.
Sanitized evidence: `docs/controller-results.json`.

Native control-plane repetition passed after safely accounting for the API's exact
standard token projection and omitted `hostNetwork: false`. Unexpected host
networking, extra containers and nonstandard token audiences remain conflicts.

Ordinary registry/gateway/proxy handover passed after six normal warmup pulls.
Native uninstall then stopped all system workloads and removed only owned serving,
RBAC and webhook resources. The fixture provider removed all six system image
references and verified their absence in the CRI inventory. API-only cached-carrier
recovery recreated serving from the same PVC UID, and ordinary operator/admission
images were pulled again. A new uncached probe ran and its exact root request was
recorded after recovery:
`sha256:1c42a2778f85eebe51549d81ad07d4b0c1f6cabe42b8842a165b7e9fd04c579e`.
Schemas, schema coordination and retained release metadata survived uninstall.

QEMU's restricted network omitted a default route. That prevented host-network
Service-IP traffic from reaching kube-proxy's translation, even while ordinary
Pod networking worked. The final fixture provisions a private gateway route before
Kubernetes starts and keeps hypervisor `restrict=on` external forwarding disabled.
An external registry connectivity probe timed out. Earlier fixture disks remain
separate failure evidence. IPv6 service/runtime support is not claimed.

The bundle now inventories and verifies Linux/amd64 static and macOS/arm64 native
installers. The packaged macOS binary executed `bundle-check` successfully. Optimized macOS/arm64 and Linux/amd64 native installers have been packaged and
verified. The default builder remains unfinished.

Continuous-read handover passed the measured manifest-read test and interrupted
installer recovery described below. Milestones 2–4 remain in scope.

### Additional real milestone-1 results

All three invalid imports ran through real quiescent producer PVCs and the native
controller: truncated bytes, a missing OCI blob and a root descriptor size mismatch.
None produced Ready=True or a verified release. See `docs/invalid-import-results.json`.
Live authorization reviews also confirmed admission's Image read permission and
its lack of Image mutation/CRD creation permission; the operator lacks CRD DELETE.

Continuous small-manifest reads passed across the read-only-server and proxy-overlap
handover: **1,186 successes, zero failures**, with OCI Accept headers. See
`docs/handover-results.json`. The first fixture probe omitted those headers and got
404s for existing OCI manifests; independent authenticated full-graph readback proved
the data remained intact. A corrected probe then found 86 real transition failures.
Forcing fresh upstream connections prevented retaining the retiring gateway through
its grace period, and the next measured run passed. This does not claim uninterrupted
large streaming transfers; a partially interrupted body still uses normal runtime
retries/digest verification. Publication is intentionally paused while the writer
changes, with no second writer; the temporary PVC mount is physically read-only.

The installer was terminated after its durable registry replacement checkpoint and
writer deletion. Resume observed the registry absent, retained the read-only serving
path, restored ordinary serving and completed. See
`docs/handover-interruption-results.json`. A separate fixture control-plane restart
caused API timeouts/TLS EOF during one retry; later native recovery resumed normally.
That interrupted run was not counted as a successful continuity measurement.

23 library tests now pass on macOS and static Linux/amd64 musl, including dual-stack
transport and nonce/configuration/credential-bound overlap proofs. Formatting and
clippy checks pass. Optimized native macOS/arm64 and Linux/amd64 installer files have
been packaged, inventory-verified, and the packaged macOS installer executes.

BuildKit v0.33.0 (built-in Dockerfile frontend v1.27.0) was prepared locally from the
upstream archive and checked against the official release asset digest:
`sha256:b6242896d343100808dcbe37565caf381e0a444a6a83d7255926bb1519248ead`.
The offline inventory/OCI verifier accepted a builder archive containing the static
daemon, buildctl and buildkit-runc. **The default worker has not yet been provisioned
or executed by the clean implementation.** Builder packaging is preparation; it is
not a completed Direct-build milestone. No complete milestone is claimed.


### Continued worker and lifecycle work

The native default-builder phase provisioned the pinned BuildKit daemon through
ordinary digest pulls, with separate server/client TLS identities, an approved
catalog, disposable cache bounds and Kubernetes Pod user namespaces. Repeated
native installation passed. A real authenticated FROM scratch/COPY solve exported
OCI content that passed independent verification; a client without its certificate
was rejected. See `docs/builder-results.json`. **Sandboxed RUN execution is still
under diagnosis and is not claimed as passing.** Initial RUN attempts exposed
missing SYS_CHROOT and an incomplete parent capability bounding set. The worker
now matches the pinned containerd executor defaults plus namespace-scoped mount
setup capabilities; subsequent init still returned EPERM and needs further work.

The builder's large layer exposed whole-layer upload timeout through the API
forwarding path. Publication now uses bounded 8 MiB Distribution PATCH chunks,
checks each returned byte range and upload origin, commits the digest, and
independently verifies all bytes. The real pinned builder graph passed upload and
readback after this change. The gateway also preserves Content-Range.

Native builder readiness now waits for an EndpointSlice naming the expected Pod
UID. Its helper retries within the same Pod IP because CNI policy programming can
lag creation; repeated install performs a fresh authenticated check. Builder and
control-plane phases use separate configuration-guarded journals, preserving the
serving handover checkpoint when adding those components.

Permanent import failures now clean up owned reader Pods, including on restart.
Obsolete unverified release metadata is deleted with UID/resourceVersion guards
without deleting registry content. A retention webhook denies published release
deletion/unpinning under the alpha permanent-pin policy; a pure policy test passes.
These latest controller/retention changes still need real API fault verification.
Fixed-cardinality process-local metrics contain no image names, paths or recipes.
25 macOS library tests pass; updated Linux checks are being run.

The owned VM was increased from 2 CPUs/4 GiB to 4 CPUs/8 GiB to reduce emulator
load. Its reboot exposed a fixture-provider systemd ordering cycle and a private
route that had been configured only for the initial boot. The private seed now
persists the route in the Kubernetes prerequisite unit and removes the diagnostic
agent's cloud-final ordering dependency. The same disk is retained. Post-repair verification preserved the exact registry PVC UID and published release
UID, restored the private route, and observed proxy upstream GET /v2/ readiness
returning 200. See `docs/fixture-reboot-results.json`; this fixture repair is
not counted as a product acceptance pass. The shared Inferal cluster is untouched.


The subsequent namespace-local NET_ADMIN correction passed the actual sandboxed
RUN solve and independent OCI verification. runc needs it while initializing the
execution namespace's loopback device; the build process keeps its smaller default
capability set and no insecure/host-network entitlement. No privileged Pod, host
volume or runtime socket was added. See `docs/builder-run-results.json`.

The updated operator/admission and retention webhook now run as ordinary digest
images. Their native control-plane phase completed after immutable component
staging using a separate seed journal. The next live fault test holds release
status writes denied at the API server, then checks generation replacement,
source-free restart recovery and retained-release deletion denial.


### Publication retry, retention and live renewal acceptance

`docs/publication-fault-results.json` records a real API-server denial of release
status writes. Neither generation became falsely Ready while sealing was denied.
The second generation replaced obsolete unverified metadata using a new UID under
the same UID/version-derived resource name; both original OCI graphs remained
readable. Readers and source PVCs were then removed and the ordinary operator Pod
restarted. Recovery retained the candidate UID, honored the durable retry deadline,
verified retained bytes and started a resolved Pod with IfNotPresent. Published
release DELETE and unpin dry-runs were denied without changing their metadata.
The fixed metrics endpoint returned a bounded, name-free response.

The initial harness incorrectly waited for the old candidate name to disappear;
replacement intentionally reuses that name. Its interrupted run was not counted as
passing. The actual fault also exposed a real status-watch feedback loop that
bypassed the computed delay. Retry deadlines now persist as generation-bound
conditions, are read authoritatively before execution, survive restart and do not
block a newer generation. The corrected live run passed.

Native `renew-tls` validated both issuer identities before writes and verified the
actual new DER leaf fingerprints served through API forwarding. A same-CA leaf
with the wrong service SAN was refused before either Secret UID/resourceVersion
changed. See `docs/tls-renewal-results.json`. Passwords, trust roots and resolution
keys were preserved; this is leaf renewal, not CA/password rotation acceptance.

The static Linux installer also performed authenticated API forwarding and full
retained operator-graph verification from an unprivileged external Linux client
container with a read-only workspace and no runtime socket. Its client network is
separate from the hypervisor-restricted Kubernetes node. Full Linux native
bootstrap/recovery of the completed bundle remains the next gate.

Additional source hardening is being verified: namespace creation/policy dry-runs,
effective reader/publisher password distinction, loopback-alias denial and canonical
proxy authority, known writable producer refusal, and PVC/PV binding identity checks
around capture. 29 macOS library tests currently pass; newer source changes will
receive the required final checks and real isolated installation/negative tests.


### Preflight and current complete bundle

`docs/preflight-results.json` records actual namespace-policy denial and a
short-lived restricted installer identity's RBAC denial before bootstrap data
writes. An incompatible schema revision marker was refused before control-plane
writes; CRD spec/UID and the existing journal were preserved, and the temporary
marker was restored. The test ran only on the owned isolated fixture.

`dist/complete-foundation-linux-amd64` includes seven components and both native
installers. Its independently supplied inventory digest is
`sha256:ca392eb16f890b9f3177a355ee612ed5680b070a4628fad6bcd3b6f30a209dc9`.
Both native macOS and packaged macOS bundle checks passed. Updated offline Linux
library checks passed 29 tests. Cold external-Linux recovery is in progress.

### Cold Linux recovery, maintenance rotation and cleanup

`docs/cold-linux-recovery-results.json` records all seven system component
references absent before recovery, blocked external access at the VM boundary,
and successful recover/seed/handover/control-plane/builder phases from an
unprivileged external Linux client with a read-only workspace and no runtime
socket. The exact registry PVC UID, three retained verified releases, and both
CRD UIDs/specs were preserved. Registry/admission trust roots and independent
reader/publisher passwords changed in the documented maintenance window.

`docs/credential-rotation-results.json` confirms new-CA verification, obsolete-CA
rejection, old reader/publisher passwords returning 401, reader writes returning
403, and a new publisher's owned upload/create/delete cycle. Explicit OCI Accept
negotiation was necessary in the test; its initial header-less 404 was a harness
error, not a missing artifact. Canonical proxy authority returned 200; alias and
arbitrary authorities returned 403.

`docs/cold-controller-results.json` records an unpredictable new root resolved to
regular and restartable-init containers with IfNotPresent and an observed runtime
root request. Capture first deferred while a known writable producer was present.
After producer termination, publication passed. Both resulting Pods remain Ready
after source PVC/Image deletion. An isolated Kubernetes API restart interrupted
the last harness wait; those final state/dry-run/alias assertions were rechecked
after readiness recovered. The restart's cause is not established by the bounded
fixture diagnostics and is not counted as product acceptance.

`docs/bootstrap-cleanup-results.json` verifies that active payload mounts prevent
cleanup, conflicting bytes remain untouched, and unmounted verified payloads are
removed with idempotent repeats. No bootstrap executable ConfigMaps remain. The
final packaged Linux installer also checked its complete bundle and repeated
cleanup. The cleanup bundle inventory is
`sha256:7ec0fe9266e81d69015647b206334bef6ffe10c85434bcf7f53048674838cf19`;
all seven runtime components and the launcher are byte-identical to the cold-tested
inventory, with only the native installers changed to include finalization.

Expanded real negatives passed truncated/missing/size/digest rejection and reader
cleanup. Wrong-platform validation correctly rejected content but exposed a
permanent/transient classification mismatch (`WrongPlatform` versus the checked
`PlatformMismatch`). The one-line correction still needs deployment and the final
permanent-failure/cleanup recheck before milestone 1 is closed.


The corrected ordinary operator digest
`sha256:a906f557dd706ead5807cee1e59a72fccb901e5dd3822a2cdd61d64c5ada4d53`
passed the actual wrong-platform permanent-failure and reader-cleanup recheck.
`docs/invalid-import-results.json` now records all five archive refusal cases;
`docs/platform-failure-results.json` records the final correction. Its development
Pod replacement is explicitly not a self-upgrade acceptance claim. The complete
updated bundle inventory is
`sha256:b40affd10a6fdb1d8623d5d2db00275bcbfb15a8cdea941f3c01137a2792646e`.
Formatting, warnings-denied clippy and 30 macOS/Linux library tests passed. The
regression exercises the actual verifier's error against controller classification.


### Milestone 1 acceptance closed; milestone 2 begins

The recovered default worker executed a real sandboxed RUN with new unique input
and independently verified OCI output:
`sha256:a48dc3c9231c9db453e0156a30d56c14b8e7b163821fd9ccc082ff46d949e492`.
See `docs/cold-builder-run-results.json`. The first repeated fixture request timed
out during ConfigMap creation; the retry with a bounded 45-second fixture client
deadline passed. Diagnostics after the same-disk restart showed NRestarts=0,
7 GiB available guest memory and 20% disk use. Earlier API resets remain a fixture
observation with no established root cause, not an assertion of product failure
or proof of HA.

Milestone 1's eight required gates have actual receipts. Milestone 2 now has pure
validated build/definition/input/strategy/cache declarations and an engine-neutral
artifact/execution contract. These are not yet wired to the source API. The public
revision-2 schema continues to reject builds. Frozen input capture, the Direct
adapter, private Secret handling, cancellation/recovery and the real build matrix
remain to implement and verify. See `docs/build-design.md`.

The in-progress revision-3 schema passed actual API-server additive dry-runs for
both CRDs without changing live revision-2 UID/spec/resourceVersion. Unit checks
accept only the shipped revision-2 definition and reject drift/downgrade. New
snapshot tests preserve binary context bytes and context-root semantics, keep
Secret definition hashes private, and reject PVC symlink escapes. The Containerfile
transport uses fixed argument arrays, authenticated worker discovery, bounded
private progress parsing, explicit no-cache and a local-only source policy.
These newer modules are not deployed or milestone-2 acceptance evidence yet.

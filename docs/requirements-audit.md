# Original brief acceptance audit

Current 0.5.1 evidence is recorded separately in the
[native acceptance](native-0.5.1-results.json) and
[real mixed-cluster acceptance](mixed-0.5.1-results.json) receipts. These cover
CLI/Helm 0.5.0 → 0.5.1 → 0.5.0 and actual Argo CD in both scopes, including
interrupted chart upgrades/downgrades, native placement, node changes and retained
OCI graph readback. Earlier unsuccessful attempts remain recorded alongside the
complete repeat; the original brief evidence below retains its historical scope.

Earlier 0.5.0 installation lifecycle evidence is separate from this original
brief audit: [native eight-case acceptance](native-upgrade-results.json) and
[real mixed-cluster CLI/Helm acceptance](mixed-upgrade-results.json) cover actual
published 0.4.1 → 0.5.0 → 0.4.1, interruption/resume, native image pulls/builds,
node eligibility/worker changes, private diagnostics and retained-data recovery.
Historical requirements and receipts below retain their original scope.

The controlling requirements are `implementation-prompt.md`, including its four
ordered milestones and required deliverables. This audit distinguishes actual
runtime evidence from unit checks and explicitly deferred extensions.

2026-10-15: all four original acceptance milestones and cross-cutting final checks
passed for the supported profile below. No required gate remains unverified in
that profile. The source, including readable colon-range selectors, passes all
49 tests on macOS and Linux/musl. The final runtime bundle's earlier 47-test
receipt is retained unchanged; it predates this selector addition.
Strict workspace/all-target Clippy passed on macOS, as did formatting and Python
syntax checks. The prepared offline Linux toolchain lacks Clippy; its full compile
and tests passed. `final-bundle-results.json` records the exact platform results.

| Original requirement | Implementation and evidence |
| --- | --- |
| Cohesive native installer and offline default dependencies | Native macOS/arm64 and Linux/amd64 installers, static launcher, eight inventoried component OCI archives and pinned BuildKit/frontend. Milestone-1 report, cold Linux recovery, final bundle verification. |
| API-only cached-carrier bootstrap and normal handover | Bounded verified executable chunks, API upload/forwarding, ordinary digest warmups, single writer/read-only transition, durable checkpoints, guarded cleanup. Foundation, handover-interruption and bootstrap-cleanup receipts. |
| Complete OCI integrity and immutable publication | Original manifest/index/config/layer bytes, SHA/size/platform/DiffID verification, topological upload and independent readback, UID/version identity and optimistic status guards. Invalid-import, platform-failure, publication-fault and completed-build recovery tests/receipts. |
| Operator-owned CRDs and compatible lifecycle | Schema phase precedes typed watches; exact owned historical 2/3 → 4 additive transitions; discovery/readiness and field/UID/resourceVersion checks; no delete/recreate or force downgrade. Preflight, Direct maintenance, export maintenance and operator-coexistence receipts. |
| Read-only coherent version resolution/admission | Exact/range/channel/prerelease semantics, readable colon and query ranges with encoded-query compatibility, same-namespace policy, signed request-bound metadata, valid/reinvocable JSON Patch, ordinary/init containers, update/subresource rejection, dry-run and direct-reference policy. Core tests include equivalent range selection and actual patch application/reinvocation for both syntaxes; actual controller and two-node delivery receipts cover the earlier runtime bundle. |
| Format/definition/strategy/engine/cache boundaries | Typed public build declarations; frozen capture and private attempt state; Direct worker lease/client/fencing modules; Containerfile/buildctl integration; standard verified OCI artifact/publication boundary. No Containerfile/LLB protocol types in registry, admission or retained export interfaces. Direct build matrix and lifecycle receipts. |
| Real definitions, input mounts and secret handling | Inline/ConfigMap/Secret/PVC definitions, text/binary/key/mode mapping, subPath, separate context/definition roots, RUN bind/secret mounts, quiescent PVC contract, selected immutable projections, source watches and private sensitive fingerprints. Direct matrix, invalid, input-change and generation-final receipts. |
| Real Local/Disabled cache and immutable release behavior | Actual engine cache-hit/miss/no-cache evidence; changed inputs invalidate; settings alone do not rebuild sealed releases; conflicting bytes cannot replace a version. Direct cache-change, version, retention and publication-fault receipts. |
| Cancellation, restart and disposable-worker failure | Durable attempt identity, renewed exclusive lease, exact client/worker UID fencing, private completion journal and trusted public root checkpoint. Final generation/deletion/restart receipts and two mocked-API recovery regressions. |
| Two-node normal uncached pulls and endpoint eligibility | Real node-targeted ordinary pulls, unchanged scheduling conjunctions, Node UID-bound admin projections, join/replacement recovery and explicit approval revocation. Two-node delivery/recovery and retained-pull receipts. |
| Registry/proxy downtime, cache loss and API recovery | Runtime cache eviction followed by normal repull, running-container preservation, uncached failure/retry during outage, both-node component-cache absence before retained-bundle recovery. Downtime and two-node retained-recovery receipts. |
| Actual binary self-upgrade and compatible rollback | `binary-upgrade-results.json`: explicit Distribution 3.1.1 ↔ 3.1.2 and serving 0.1.0 ↔ 0.2.0 window, independent trusted bundles, storage-profile refusal, uncached warmups/root requests on both proxies, writer-gap interruption, resume, newer writes read by the old binary, cold old-bundle recovery and Linux-native roll-forward. `serving-metrics-final-results.json` records native rollout of the later proxy-only HTTP metrics correction. Packaging-only evidence is historical and does not establish this gate. |
| Independent optional authenticated exports | Separate exporter identity over the immutable retained UID/root, no live worker/controller dependency, exact remote graph/tag readback, conflict/idempotence, restart/retry, password rotation, remote outage independent of internal publication/pulls, removal without remote deletion. Export/RBAC/boundary receipts; `export-after-upgrade-results.json` repeats independent status and retained export with the publisher stopped after the binary cycles and final correction. |
| Retention and uninstall safety | Permanent alpha release pins/indefinite grace, detached metadata, no automatic backend GC or remote deletion; ordinary uninstall retains CRDs/PVC/releases. Actual deletion-denial, source-free resolution and maintenance recovery receipts. |
| Bounded observability and sanitized diagnostics | Fixed-cardinality latency/active/failure/transfer counters, concurrency waiters, catalog readiness/pending counts, endpoint-readiness sampling, freshness/completeness gauges and contextual logs. Stream-abandonment and real HTTP fixed-length/truncation regressions; `observability-results.json` verifies increases from an actual build/publication/pull, exact catalog counts, both eligible nodes and no false successful-pull failures. |
| Operational deliverables | Architecture, build/export design, operations/backup/renewal/retention/recovery guides, generated versioned schemas, native resource generation, reproducible live fixture scripts, per-milestone reports and this audit. |

## Supported profile and explicit limits

The runtime baseline is Linux/amd64, Kubernetes v1.36.5+k3s1, containerd
2.3.4-k3s1.36, Distribution filesystem storage and BuildKit 0.33.0/builtin
Dockerfile 1.27.0. Native clients are macOS/arm64 and Linux/amd64 musl. All new
destructive/fault/cache tests use the owned disconnected QEMU cluster; the retained
Inferal PoC resources and shared nodes are untouched.

The first build adapter uses scratch and provisioned local inputs, as required
for the disconnected acceptance fixture. Remote bases/frontends/ADD/context and
build network access are refused. The initially supported external registry
authentication profile is verified HTTPS Basic or explicit anonymous push, with
exact-authority dockerconfigjson credentials and optional private CA. Registries
requiring Bearer/token-helper login are outside that interoperability profile;
the GHCR repository name in the brief is a declaration-shape example, not a claim
that its authentication flow was exercised. These restrictions are part of the
documented initial adapters rather than a claim of arbitrary registry/build support.

The brief expressly leaves future production formats/strategies/builders/shared
caches and signing/attestation policy to later acceptance. Multi-platform graphs,
hostile-tenant isolation, registry HA, automatic retained-artifact GC, destructive
purge, arbitrary binary downgrade and CRD/storage-version migration are unsupported.
No unavailable optional extension is counted as a passed runtime test.

## Final artifact and state

The complete eight-component bundle is `dist/complete-final-linux-amd64`, inventory
`sha256:d49c04fbae9582a4fda78db8974b2698cc6ad1fb651294cfd3f42dbdd70a0f3e`.
Both packaged native installers independently verified it. It retains every
artifact from the full binary-lifecycle-tested bundle except the proxy, whose
HTTP metrics correction has its own native rollout and live pull/metrics receipts.
The independently retained `dist/complete-rollback-linux-amd64` has matching
non-serving roots and the supported older serving binaries.

`final-health-results.json` verifies two Ready nodes, all seven expected
serving/control/exporter Pods Ready at exact final component digests, and the
preserved registry PVC UID. Published release and CRD identities/specs survived
the recorded upgrade, rollback and recovery cycles. The `main` branch contains
the implementation; the historical PoC orphan branch is preserved locally.
The runtime bundle and live receipts predate the source's readable colon-range
addition, which has separate parser, catalog and admission regression coverage.

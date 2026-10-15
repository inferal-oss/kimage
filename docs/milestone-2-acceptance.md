# Direct build acceptance

2026-10-14. **Milestone 2 passed for the pinned single-node baseline.** The core
matrix and the final generation-cancellation, Image-deletion and operator-restart
repetitions passed against the last cleanup fix. Multi-node delivery, product
self-upgrade/rollback and external export results are in the separate milestone-3
and milestone-4 reports.

## Evidence

| Gate | Actual evidence |
| --- | --- |
| Public typed build API and publication | `direct-build-results.json`: revision-3 `source.build.format: Containerfile`, Direct/default placement, real pinned BuildKit execution, independent OCI verification, retained release, admission resolution, normal `IfNotPresent` workload startup and a proxy root request. |
| Definitions, paths and mappings | `direct-build-matrix-results.json`: inline and mounted ConfigMap/Secret/PVC definitions; binary/text/key/mode mapping, definition subPath, separate definition and COPY roots, PVC subPath context, known-writer refusal and recorded PVC/PV UIDs. |
| RUN isolation and secret sessions | The real RUN fixture checks a read-only ordinary bind mount, executable mode and a separate secret mount. The worker uses the pinned namespace-root profile with Pod user namespaces, process sandbox, fixed capabilities, no privileged/host-volume/runtime-socket access, network none and denied egress. |
| Actual cache behavior | Matrix receipts report Local miss `0 cached / 1 executed`, Local hit `1 / 0`, and Disabled `0 / 1`. `direct-build-cache-change-results.json` reports an actual Local RUN miss after ordinary bind input bytes change. A cache-mode change preserves the existing release and execution receipt. |
| Frozen inputs and stale work | `direct-build-input-change-results.json`: the API refuses mutation of the immutable selected snapshot; changing the original ConfigMap during an in-use real RUN fences the old worker and publishes the new resource version. Generation cancellation has its own worker/attempt receipt. |
| Invalid inputs and private definitions | `direct-build-negative-results.json`: unsupported format/strategy/builder/platform, mixed definition/source selectors, traversal, overlapping mounts, hostPath/mount-expression controls, missing key/file and external syntax frontend are refused. A malformed Secret definition fails without its text or sensitive fingerprint in public status or normal daemon/controller logs. |
| Immutable publication and lifecycle | `direct-build-version-results.json`: source change without Image generation change produces `VersionConflict`, preserves the first release UID/spec, and publishes changed content under a new exact version. Generation/deletion/restart receipts prove in-use RUN fencing; deletion completes its finalizer without a stale release. |
| Retention and source-free publication recovery | `direct-build-retention-results.json`: 39 ready samples across worker/cache replacement with the release and execution unchanged; a retained Pod then starts after source objects/PVC/Image/worker/cache removal. `direct-build-publication-fault-results.json`: a targeted API seal denial cannot publish readiness; after client/source/worker removal and operator restart, the same candidate UID/root/execution seals through independently verified retained registry bytes. The temporary webhook is removed. |

The cancellation diagnostic reads live in-use execution storage records privately.
Build history stays disabled. An earlier diagnostic that relied on disabled build
history did not establish an in-flight cancellation and is not counted as a pass.

## Versions and deployment lineage

The fixture remains Linux/amd64 Ubuntu 24.04.5, Kubernetes `v1.36.5+k3s1`,
containerd `2.3.4-k3s1.36`, Distribution `3.1.2`, BuildKit `0.33.0`, built-in
`dockerfile.v0` frontend `1.27.0`, and Rust `1.99.0`. Guest public-registry access
remains blocked at the hypervisor. No host runtime socket, registry mirror,
node-CA configuration or application image-store injection is used.

Native maintenance recovery installed inventory
`sha256:01e46f28cd60f6e69b680c9a200262e0a38988deebb1e7b8f620fe7d22f86de2`.
It preserved the registry PVC, retained release UIDs/specs and both CRD UIDs while
the operator upgraded the exact shipped revision-2 schemas to revision 3 in place.
This was a maintenance deployment, not product self-upgrade acceptance.

The last development operator root is
`sha256:e749b74764de4f3aae9028e25c07506dae4d01cab68a9d205f9f57ab7f1f48d2`,
from inventory
`sha256:6e91a4168df70052d5ee02fbfe3666afe11df8276bc7d236b0e9309163d2b319`.
Its ordinary Pod replacement, live missing-client recovery regression, second
mounted-input matrix, retention and API-seal recovery have separate receipts.
Earlier lifecycle receipts used the preceding operator with the same fencing
protocol. The final repetition passed against the last cleanup implementation; see
`direct-build-generation-final-results.json`, `direct-build-delete-final-results.json`
and `direct-build-restart-final-results.json`.
The revision-3 definitions are retained in `crates/control/schemas/revision-3.json`.

## Checks and limits

`cargo fmt --all --check`, strict workspace/all-target Clippy, Python compilation,
and 38 meaningful tests pass on macOS and Linux/amd64 musl. The two API regressions
cover a completed journal whose client disappeared and rejection of a private
journal root without the trusted public verified-root checkpoint. Real tests
found and fixed invalid optional defaults, cleanup races, missing-client recovery
and worker-loss readiness regression. Mocks are not the runtime acceptance proof.

The initial adapter supports scratch plus provisioned local inputs. Other formats,
strategies, builders, remote bases/frontends/caches and hostile tenant isolation
are unimplemented. The single application namespace is the worker/cache trust
scope. Secret definition literals can enter image history, cache or deliberate
recipe output; actual credentials belong in secret mounts. Secret values do not
implicitly invalidate cache; the explicit non-sensitive token conservatively
disables cache when a solve uses it.

PVC capture relies on producer handoff and identity/writer checks, not an atomic
filesystem snapshot. Storage and GC targets include eventual kubelet enforcement.
The emulated four-CPU fixture developed a CPU queue and one conservative lease
loss; that attempt retried without a false acknowledgment. The health snapshot
showed no service restart or kernel OOM, with memory/disk headroom. These are
correctness receipts, not throughput, HA, supply-chain signing or reproducibility
claims. No retained artifact garbage collection or remote deletion is performed.

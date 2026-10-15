# Milestone 1: single-node acceptance

The eight required gates passed on the pinned baseline. Later milestones are
recorded in their separate acceptance reports. This report describes composed acceptance evidence across successive
trusted bundles and retains the scope of each receipt.

| Required gate | Actual evidence |
| --- | --- |
| API-only macOS/Linux install, bounded carrier delivery, handover, interruption/resume and cleanup | Native macOS foundation/controller runs; `cold-linux-recovery-results.json` covers the unprivileged external Linux client and all five native phases. `handover-interruption-results.json` covers a killed installer after durable replacement intent; `bootstrap-cleanup-results.json` covers active-mount refusal, mismatched-byte preservation, removal and idempotent repeats. |
| Original OCI graph → verified release → fresh normal platform Pod pull | `controller-results.json` and `cold-controller-results.json` record unique roots, actual proxy manifest requests, IfNotPresent, regular/restartable-init containers and retained operation after source/Image deletion. |
| Untouched runtime/node trust and real endpoint/auth/TLS behavior | Loopback read-only proxy, verified authenticated HTTPS upstream, ordinary kubelet pulls. `credential-rotation-results.json` covers 200/401/403/202/204 behavior, obsolete trust rejection and canonical Host enforcement. `tls-renewal-results.json` covers live leaf reload and wrong-SAN refusal before Secret writes. |
| Invalid artifacts, interruption, duplicate/replacement attempts and restart never create false readiness or remove retained graphs | Five cases in `invalid-import-results.json`; final platform correction in `platform-failure-results.json`. `publication-fault-results.json` covers real API denial, generation replacement, preserved graph bytes, source-free controller restart, durable retry and retention denial. Same-PVC recovery preserved prior release identities and schemas. |
| Read-only dry-run and valid Pod mutation | Controller receipts plus patch-application/reinvocation/forgery unit tests; unmanaged carrier policy is preserved, managed policy/annotation fields are inserted and init sidecars resolve. |
| Operator provisions absent CRDs before typed watches and handles incompatibility safely | Original isolated controller installation created revision-2 CRDs. `preflight-results.json` covers actual RBAC, admission and incompatible-revision refusal without bootstrap/control writes, with CRD UID/spec preserved. Unit checks cover definition/field-ownership guards. |
| All-components-down recovery with component images absent | `cold-linux-recovery-results.json`: all seven target references absent, same registry PVC UID, three retained verified release UIDs/specs preserved, and recover/seed/handover/control-plane/builder passed. A new unpredictable platform image then pulled normally. |
| No external registry needed for exercised install/worker/pull sequence | QEMU hypervisor restrict mode and an external-access deadline are distinct from Pod NetworkPolicy. `cold-builder-run-results.json` records a real sandboxed RUN and independently verified OCI export on the recovered default worker. |

The isolated baseline is Linux/amd64, Ubuntu 24.04.5/kernel 6.8.0-142-generic,
Kubernetes v1.36.5+k3s1, containerd 2.3.4-k3s1.36, Distribution 3.1.2, BuildKit
0.33.0/builtin Dockerfile 1.27.0, Rust 1.99.0, kube 4.2.0 and k8s-openapi 0.28.0.
macOS/arm64 and static Linux/amd64 native installers are packaged. Cargo is locked.
Formatting, warnings-denied clippy and 30 library tests passed on macOS/Linux
before the separate milestone-2 contract work began.

The complete cold-tested inventory is
`sha256:ca392eb16f890b9f3177a355ee612ed5680b070a4628fad6bcd3b6f30a209dc9`.
The cleanup inventory changed only the installers; the final platform correction
changed only the operator runtime component. The final coherent bundle is
`dist/platform-failure-linux-amd64`, inventory
`sha256:b40affd10a6fdb1d8623d5d2db00275bcbfb15a8cdea941f3c01137a2792646e`.
All its component executables match the completed offline Linux release build.
The corrected operator was staged/verified and pulled as an ordinary digest Pod;
that explicit development replacement is not a product self-upgrade claim.

One registry/PVC is authoritative, with no HA or automatic blob GC. Published
releases have permanent retention pins; force/admin bypass remains outside the
guarantee. API recovery requires a usable API/kubelet/carrier/PVC. Producer handoff
does not create an atomic filesystem snapshot. The worker uses a Pod user
namespace, namespace-scoped capabilities and a process sandbox; hostile tenant
isolation is not proven. Credential CA/password rotation uses a maintenance
uninstall/recover window; same-CA leaf renewal reloads in place.

The emulated fixture had API timeouts/resets during follow-up checks. Tests were
resumed/rechecked rather than counting failed waits as passes. Bounded diagnostics
did not establish their cause; the same disk and retained data were preserved.
The original Inferal PoC PVC, Images/releases, CRDs and shared nodes were untouched
throughout this isolated acceptance sequence.

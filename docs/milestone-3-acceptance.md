# Milestone 3 acceptance

Passed for the explicit binary compatibility window below. Real Distribution
3.1.1 → 3.1.2 and gateway/proxy 0.1.0 → 0.2.0 upgrades, reverse rollback and
cache-absent retained-bundle recovery passed. The final roll-forward used the
packaged unprivileged native Linux installer with verified API TLS.

The owned two-node fixture runs Kubernetes v1.36.5+k3s1 and containerd
2.3.4-k3s1.36 on Linux/amd64. Two QEMU guests have a dedicated private Ethernet
link and individually restricted SLIRP uplinks. Cluster networking, K3s agent
joining and the cached pause carrier are fixture prerequisites, separate from the
product. kimage changes no node registry mirrors, CA trust, runtime stores or host
configuration. No shared Inferal resource is used by these tests.

| Gate | Current evidence |
| --- | --- |
| Two uncached normal pulls and controller rescheduling | Passed: `two-node-delivery-results.json` records a fresh retained root absent from both CRI caches, normal IfNotPresent pulls and proxy requests on both nodes, a new Deployment-controlled Pod on the second node, unchanged release/execution, and preserved user scheduling/annotations. |
| New node eligibility | Passed: native `join-node` installs the proxy through API-carried executables, a normal warmup pull and ordinary proxy handover. Administrator-owned endpoint projections bind node UID, scope, pinned image and serving profile; Ready Pod labels alone do not authorize a node. Explicit approval revocation excludes the node. |
| Node replacement and retained artifact recovery | Passed: `two-node-recovery-results.json` records a new Node UID, refusal of its stale approval, idempotent native proxy recovery, actual CRI eviction and normal repull without release/execution changes. |
| Proxy and registry restart | Passed: the same receipt records distinct serving Pod UIDs and an unchanged running application container ID. |
| Registry downtime | Passed: `registry-downtime-results.json` records preserved running container/readiness, an uncached normal pull failing while serving is down, and the same Pod recovering through normal retry after registry restoration, without rebuilding. |
| Real serving binary self-upgrade, interruption and compatible rollback | Passed: `binary-upgrade-results.json` records different actual executable versions/hashes, target images absent before warmup, exact replacement root requests on both old proxies, interruption with the writer absent, an uncached retained normal pull through read-only serving, resumed upgrade, actual version commands, fresh publication under 3.1.2 and an uncached pull of those new bytes after rollback to 3.1.1. |
| Compatible old/new operators and additive CRDs | Passed: the actual revision 2 → 3 additive transition and restart recovery passed milestone 2; `operator-coexist-results.json` records two distinct compatible operator binaries Ready together, successful publication, restart of the old binary, and unchanged schema identities/specs. |
| Ordinary uninstall and retained-bundle API recovery | Passed: `two-node-retained-recovery-results.json` records both component caches absent before native API-only recovery, restored additional-node proxy, and unchanged PVC/release/CRD identities. `two-node-retained-pull-results.json` records a fresh uncached normal pull on node 2 after deleting the source ConfigMap and Image; the retained release resolved through admission without rebuilding. |
| Older-bundle recovery after newer binary writes | Passed: `binary-upgrade-results.json` records all eight component references absent from both node caches, API-only recovery through the older 3.1.1/0.1.0 bundle, preserved retained identities/specs, independent full newer-graph verification through the older registry, Linux-native roll-forward and another independent full graph verification. |

The real two-node test found and fixed a Kubernetes interoperability error in the
single-node affinity mutation: a metadata.name field selector accepts one value,
not a list. Admission now distributes eligibility over the user's OR terms,
keeps each conjunction, preserves empty unschedulable terms, bounds expansion,
and handles reinvocation without multiplying terms. Unit tests and actual API
admission/controller scheduling passed after the fix.

The workspace includes compatibility/refusal, storage-profile, OCI/admission,
HTTP-stream accounting and API-recovery regression tests. Exact final test counts
and platform results are recorded in `requirements-audit.md` and
`final-bundle-results.json`.

The updater supports the shipped Distribution filesystem profile for 3.1.1 ↔
3.1.2, and gateway/proxy 0.1.0 ↔ 0.2.0 with the unchanged v1 authenticated
overlap protocol. It refuses other binary versions, storage/configuration
overrides, bundle schema mismatches and changes to non-serving component roots.
No data migration or CRD rollback occurs. The old `serving-upgrade-results.json`
is historical packaging-only evidence; it is superseded for binary compatibility
by the two real cycles. See `binary-upgrade-design.md` for exact scope and the
reproducible harness. Distribution 3.1.2 remains the ordinary-operation default.

Multi-node delivery does not provide registry HA. The single retained PVC and its
node/attachment topology still limit registry placement and failure recovery.
Endpoint approval is an administrator capability. Admission reads it and actual
node/Pod readiness; it never bootstraps a node or builds/publishes content.

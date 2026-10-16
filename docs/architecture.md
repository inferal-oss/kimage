# Clean implementation architecture

As of 0.4.0, the CLI and Helm apply ordinary resources with multi-platform GHCR
image pins. System components pull independently of the included application
registry. See [installation](installation.md). The bootstrap, carrier and bundle
recovery entries below record the earlier implementation and acceptance evidence.


For 0.3.0 platform selection, default on-demand production, eager configuration,
and namespace/cluster installation, see [native platforms and scope](platforms-and-scope.md).
Earlier acceptance receipts below describe the historical amd64 fixture; native
Kubernetes E2E now covers both architectures and both installation scopes.

Decision date: 2026-10-14. The experimental branch is evidence, not the
implementation baseline. Acceptance updated 2026-10-15: the original four
milestones and cross-cutting observability checks passed for the documented
profiles. Real serving upgrades/rollback cover Distribution 3.1.1 ↔ 3.1.2 and
gateway/proxy 0.1.0 ↔ 0.2.0, with interruption and older-bundle recovery.
See the requirements audit and acceptance reports for the exact binary and HTTPS
authentication windows; arbitrary storage/schema/binary migration and HA remain
unsupported.

## Initial review

| Problem and consequence | Correction | Verification |
| --- | --- | --- |
| Registry publication and Kubernetes status are separate transactions; a crash can leave partial blobs or false readiness. | Validate a complete original OCI graph first. Persist operation identity, upload children before roots, independently read back bytes, then create an immutable release. Resume idempotently; never delete partial/shared blobs from reconciliation. | Corruption, duplicate/concurrent publication, fault injection between phases, registry restart and stale-generation tests. |
| A version scoped by name can collide after Image recreation. | Scope version identity/repositories by Image UID; deterministic release name; compare immutable content on conflict. Retain releases independently of their Image. | UID recreation, conflict and concurrent-create checks. |
| Cached images hide broken endpoints and bootstrap dependency cycles. | Cached carrier/static launcher only for bootstrap. Ordinary workloads pull digest references through loopback proxy with verified HTTPS upstream. Endpoint eligibility is independent of release verification. | Fresh unique image plus real kubelet requests; all-components-down recovery in isolated cache-absent fixture. |
| Registry images cannot bootstrap their own registry, and one RWO writer limits handover. | API-only bounded executable delivery and port-forward upload; trusted offline bundle with component/version inventory. Persist installation phase and resource ownership; warm replacements before a single-writer transition. | Interrupt/resume each install phase, carrier absence, policy denial, PVC identity preservation. A stop/start transition has downtime; do not claim uninterrupted service. |
| A Lease does not prevent an old operator from downgrading schemas. | Built-in schema phase before CR discovery; revision/definition/ownership guard, optimistic writes, no force apply or deletion. Additive changes require explicit compatibility; refuse unsupported conversion/storage migration. | Actual establishment/discovery, foreign ownership, additive upgrade and old/new binaries in isolated fixture. |
| Opt-in webhook labels allow bypass, while self-hosting can deadlock admission. | Administrator-controlled system exclusion; fail-closed declared application scope; same-namespace consumption; direct internal references checked too. Read-only catalog with bounded demand recording in admission; CREATE-only resolution, explicit update/ephemeral rejection. | Actual API-server dry-run and reinvocation, JSON Patch application, outage/routing and forgery tests. |
| Independently listed Images/releases can select incoherent channel/UID state. | Versioned coherent catalog snapshot, only verified immutable releases, explicit channels, strict custom URI grammar and semantic precedence. | Channel races, deleted/recreated Images, ranges/prereleases and invalid URI matrix. |
| Mutable projections/read-only PVC mounts are not snapshots. | Capture ConfigMaps/Secrets at explicit UID/resourceVersion; frozen reader workspace; producer handoff required for PVC. Secret input stays outside COPY context and public fingerprints. | Mutation during capture, binary/mode/key mapping, PVC/subPath, mount/path escape and Secret definition/session checks. |
| Dropping a transport process is not durable worker cancellation. | Persist attempt identity separately from logical input identity before submit. Stale generations cannot acknowledge current work. Define cancel/retry recovery at the strategy boundary. | Crash/restart and cancellation during capture, solve and publication; changed-input cache checks. |
| BuildKit/frontend/cache implementation can leak into release APIs. | Separate format/definition, strategy, approved builder catalog and cache policy. Standard OCI result enters common verifier/publication. Direct uses pinned buildctl argument arrays and isolated worker. | Real inline/mounted builds, RUN bind/secret mounts, Local/Disabled, retention after worker/cache removal. |
| Shared credentials and node access imply broader trust than admission authorization. | Separate publication and read identities; fixed upstream digest-only loopback gateway, verified TLS, no redirects/token forwarding or node edits. Trust eligible node administrators as readers. | Read identity cannot write, bad credentials/CA, Range/HEAD behavior, reload/rotation tests. |
| Self-upgrade and cache eviction can strand serving components. | Warmup normal pulls on each target node, bounded rollout, previous independent trusted bundle and API recovery; no schema downgrade on binary rollback. | Two-node uncached pulls/new-node eligibility, interrupted rollout, proxy/registry restart, cache eviction in isolated fixture. |
| Export failure can accidentally affect internal readiness. | Independent milestone-4 adapter over retained verified bytes, pinned release UID/root, destination-scoped credentials and per-target retries. No remote deletion. | Second registry, original digest readback, tag conflicts/races, rotation and outage without rebuild. |
| Automatic GC or uninstall can destroy retained releases. | Alpha never automatically deletes published metadata/content. Ordinary uninstall retains CRDs/PVC/releases; deny explicit release deletion pending implemented pin/usage protocol. Backup entire backend with release metadata. | Uninstall/restore, shared-layer retention and input/Image deletion tests. |

## Decisions, upstream facts and limits

The PoCs confirmed Kubernetes `v1.36.5+rke2r1`, containerd `2.3.4-k3s1`,
Linux/amd64, Distribution `3.1.2`, BuildKit `v0.33.0` and builtin Dockerfile
frontend `v1.27.0`. The clean implementation initially targets those versions;
new code still needs its own interoperability receipts. Rust is `1.99.0`,
`kube` `4.2.0`, `k8s-openapi` `0.28.0` with `v1_36`. Cargo dependencies are locked.
The default builder is an administrator-managed OCI/runc worker with native
snapshotter; hostile multi-tenant isolation is not claimed.

[OCI image descriptors](https://github.com/opencontainers/image-spec/blob/v1.1.1/descriptor.md)
bind original bytes by media type, size and digest. The
[Distribution protocol](https://github.com/opencontainers/distribution-spec/blob/v1.1.1/spec.md)
provides supported blob uploads and manifest writes; it does not make registry/API
publication atomic. Preserve graph bytes, roots, compressed layers and DiffIDs.
Do not rewrite the registry filesystem or invent a runtime import API.
[Distribution 3.1.2](https://github.com/distribution/distribution/releases/tag/v3.1.2)
is separately packaged; its inventory authenticates integrity only after origin
has been established through a trusted delivery channel.

One PVC/registry is authoritative, not highly available. Registry outage leaves
running containers alone but can prevent uncached starts. RWO attachment and
producer quiescence are prerequisites. Kubernetes API/kubelet/CNI/storage,
namespace policy, allowed per-node cached carrier and internal network are explicit
dependencies. An unusable node or lost PVC cannot be repaired by API-only recovery.
Pod NetworkPolicy does not prove host-runtime disconnection. Cache-absent,
disconnected and incompatible-schema checks belong in isolated fixtures.

No hardcoded test node, namespace, address, date or opt-in label becomes a product
contract. Retained PoC PVC/releases/CRDs are preserved; the clean operator must
refuse incompatible existing schemas rather than silently adopting them.

## Ordered implementation ledger

1. First implement the shared OCI verifier/publication path, immutable resource
   model, durable operation checks, read-only selection/admission, guarded schema
   startup, native bundle/installation state and fixed loopback gateway.
2. After complete single-node installation/runtime acceptance, implement frozen
   input capture and Direct/Containerfile against the provisioned default worker.
3. After build acceptance, implement multi-node eligibility and safe self-upgrade.
4. Then implement independent retained-artifact exports.

No acceptance gate is amended or waived. Each phase records implementation,
local checks and live/isolated evidence separately in `docs/status.md`.

### Read availability during single-writer handover (validated baseline)

A temporary ordinary-image Distribution instance mounts the existing PVC read-only,
sets `storage.maintenance.readonly.enabled: true`, and disables upload purging. Its
TLS gateway is independently read back before the Service routes to it. The old
writer is stopped before the new writer starts; publishing can retry during this
interval. This uses the implementation's [documented read-only maintenance mode](https://github.com/distribution/distribution/blob/v3.1.2/docs/content/about/configuration.md),
plus the kernel read-only mount, without editing registry storage layout.

New proxies prepare SO_REUSEPORT and require an explicit overlap request plus a
fresh nonce proof tied to the same upstream, namespace allowlist, CA and reader
credential before joining an occupied loopback endpoint. Initial installation
refuses an occupied port. Node host-network administration remains a trusted
capability. A GET/HEAD connection failure can retry before response headers; an
already streamed interrupted transfer still relies on the runtime's normal retry
and digest verification. Live interruption and continuous-read tests are pending;
no HA or uninterrupted large streaming transfer guarantee is claimed.

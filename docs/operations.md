# Operating kimage

This implementation is still under development. `kimage-install`
boots/seeds/hands over registry serving and provisions the operator, admission
configuration and their separate RBAC identities. Dedicated `builder`, `join-node`,
`upgrade-serving` and optional `exporter` phases passed their documented fixture
gates. Consult the
[acceptance ledger](status.md) before treating a command as a milestone gate.

## Official release bundles

Install the native CLI without a checkout or packaging script:

```sh
cargo install kimage-control --version 0.2.0 --locked
kimage install --help
```

Bundle-using phases default to the exact release matching the CLI's package
version, never `latest`. They reuse a verified private cache on later runs.
`--release 0.2.0` pins a different exact selection, `--bundle-cache <directory>`
overrides the cache, and `--offline` prevents bundle downloads and fails on a cache miss. Fetching
requires no kubeconfig and performs no cluster writes. GitHub Actions publishes
the complete Linux/amd64 bundle, inventory, native macOS/arm64 and Linux/amd64
installers, license notices, and checksums as release assets.

The HTTPS GitHub API at the fixed official `inferal-oss/kimage` repository
provides the expected asset digest/size. The client restricts HTTPS download
origins, verifies the archive and inventory, rejects unsafe archive entries,
checks static executable headers and full OCI graphs, and atomically installs
only a complete cache entry. Cached files are reverified before reuse; partial
or invalid cache entries never become installation inputs. GitHub authentication
tokens are not needed for public downloads. Optional `KIMAGE_GITHUB_TOKEN` applies
only to the fixed metadata endpoint for API rate limits, never asset downloads or
redirects. Keep an independently retained copy for
recovery if GitHub is unavailable.

For an air gap, transfer the fetched `bundle` directory and its inventory digest
through your approved channel. The original `--bundle <directory>` plus
`--trusted-inventory sha256:<64-hex-digits>` path remains supported without a
network request. A local bundle always requires that independent pin. Official
bundles start at 0.2.0; the 0.1.0 release has no component bundle assets.

## Standard installation

Once the cluster and credentials described below are prepared, use:

```sh
kimage install \
  --runtime-profile k3s \
  --namespace kimage-system --application-namespace apps \
  --installation dev --node <approved-node> \
  --carrier <cached-image@sha256:digest> \
  --storage-class <class> --port <loopback-port> \
  --credentials <registry-directory> \
  --admission-credentials <admission-directory> \
  --builder-credentials <builder-directory>
```

Bundle download, verification, caching, bootstrap, seed, handover, control plane,
and default builder setup run automatically. No separate fetch/check step is
required. Credential directories for all phases are checked before bootstrap
writes. Rerun with identical arguments to resume; existing ownership, configuration,
and bundle checkpoints are checked before selecting a phase. Partial handovers
resume from their replacement checkpoints, rather than restarting bootstrap.
`kimage-install` remains a compatible alias for all commands. Individual phase
commands below remain useful for recovery or advanced installation.

For optional prefetch before kubeconfig is available or an air-gap transfer:

```sh
kimage bundle-fetch
kimage bundle-check --offline
```

Here `--offline` prevents bundle downloads only. Cluster commands still require
access to the Kubernetes API.

## Trusted bundle and credentials

Normal administrators use the official fetch path above. The following packaging
commands are optional source-build tooling for maintainers and custom bundles.
Build Linux/amd64 static binaries in a prepared
Rust 1.99.0/musl toolchain, with dependency cache available offline:

```sh
cargo build --locked --offline --release --target x86_64-unknown-linux-musl --workspace --bins
python3 scripts/package-bundle.py \
  --linux-dir <linux-release-directory> --registry <trusted-distribution-3.1.2-executable> \
  --registry-version 3.1.2 --serving-version 0.2.0 \
  --buildkit-dir <pinned-buildkit-directory> \
  --include-installers --macos-installer <native-macos-arm64-installer> \
  --out dist/complete-linux-amd64
```

Authenticate the executable/bundle origin through a trusted delivery channel.
Provide the inventory SHA-256 from that channel via `--trusted-inventory`; reading
an adjacent checksum alone does not establish origin. The packer rejects dynamic
interpreters and wrong architecture. Native verification rechecks bytes, bounded
ELF headers and every component's standard OCI graph before cluster mutation.
Launcher is at most 512 KiB; executable chunks are at most 512 KiB each, bounded
to 256 chunks/128 MiB expanded, and encoded objects are capped at 768 KiB.
Only executables, configuration and bounded metadata go into ConfigMaps.

Supply an issuer-managed credential directory with `ca.crt`, server `tls.crt`,
`tls.key`, distinct `reader-password` and `publisher-password`. The leaf must
authenticate `<installation>-registry.<namespace>.svc`; do not serve a CA as an
end-entity certificate. Keep the CA private key outside Kubernetes. Credentials
never go into OCI archives. `scripts/prepare-test-credentials.py` is an explicit
local seven-day development issuer requiring OpenSSL; it is not production
certificate lifecycle automation.

## API-only bootstrap and delivery

Supply either the automatic release selection or explicit local bundle/pin to
each phase. Use an administrator kubeconfig and explicitly selected Linux/amd64 node, cached
approved carrier digest, StorageClass (or `--pvc` for an existing claim), installation
namespace/name and nonprivileged loopback port. Preserve the same arguments for
resume. Config changes or foreign resource collisions are refused.

```sh
kimage-install bootstrap --release 0.2.0 \
  --namespace <system-namespace> --application-namespace <application-namespace> \
  --installation <unique-name> \
  --node <approved-node> --carrier <cached-image@sha256:digest> \
  --storage-class <class> --port <loopback-port> --credentials <directory>
```

The carrier uses Never only for the bootstrap registry/gateway/proxy. It executes
the projected static launcher with explicit arguments, without a shell or carrier
tools. Registry scheduling uses required node affinity so WaitForFirstConsumer
storage can bind. No SSH, runtime socket, host filesystem or trust-store edits
are involved. The installer creates missing requested namespaces, preserves existing
namespace policies, and uses authorization reviews plus admission dry-runs before
creating bootstrap data/Secrets. Carrier absence is a prerequisite failure; there is no external pull
fallback. Admin RBAC does not override namespace admission policy.

`seed` with the same arguments uploads blobs/child manifests before roots through
authenticated API port-forwarding, then independently reads back original bytes.
`handover` warms component images through normal kubelet pulls before replacing
serving Pods with digest images. Replacement intent is checkpointed before deletion;
resume can recreate a missing registry from that checkpoint. A temporary server
mounts the PVC read-only while the single writer changes; compatible loopback
proxies overlap using an authenticated configuration proof. Continuous manifest
reads passed in the isolated fixture. Publication pauses during replacement;
interrupted large response bodies still depend on runtime retry/digest checks.
The latest handover finalizes temporary executable ConfigMaps after ordinary
serving readiness; the explicit `cleanup` phase can resume this guarded deletion.

`control-plane` additionally requires `--application-namespace` distinct from the
dedicated system namespace, and `--admission-credentials` with an independent
service TLS identity and resolution key. It waits for operator-owned schemas,
ordinary operator/admission Pods and independently verified TLS before registering
the application webhook. An incompatible existing schema is refused before writes.

The official release bundle already includes the builder. Custom bundle packaging
uses `--buildkit-dir` containing the official pinned
BuildKit v0.33.0 static `buildkitd`, `buildctl` and `buildkit-runc` executables. Native
installers can be inventoried with `--include-installers --macos-installer <path>`.
After seeding, `builder` uses the same installation arguments plus
`--builder-credentials <directory>` containing `ca.crt`, `server.crt`, `server.key`,
`client.crt` and `client.key`. The server SAN is
`<installation>-builder.<system-namespace>.svc`; client and server have separate
extended key usages. The issuer private key stays outside Kubernetes.

The worker requires Linux idmapped volume support and Kubernetes user namespaces.
Its root UID is mapped away from host root (`hostUsers: false`); selected mount/user
capabilities apply inside that namespace. Nested runc mount-namespace entry needs
both SYS_ADMIN and SYS_CHROOT, as specified by the
[Linux setns interface](https://man7.org/linux/man-pages/man2/setns.2.html). It keeps process sandboxing enabled,
uses the native snapshotter, and has no host/runtime mounts or service account
token. Seccomp/AppArmor are unconfined inside this worker Pod to permit nested
execution; this baseline is administrator managed, not a hostile tenant service.
Unsupported namespace/kernel policy fails without falling back to a privileged
Pod. The engine's host network mode refers to the worker Pod network namespace.
A dedicated NetworkPolicy denies worker egress; its enforcement depends on the
cluster's CNI. The fully disconnected fixture additionally blocks external
forwarding outside the guest, including runtime traffic.

Cache is an independent disposable emptyDir with a 4 GiB volume limit, 2 GiB GC
target, one parallel execution step, 1 CPU/1 GiB worker limits, and bounded helper
deadlines. The 4 GiB emptyDir limit is enforced through kubelet eviction, not a
synchronous write quota; GC is an eventual target. Registry artifacts use a
different retained PVC. Build history, gateway frontend execution, CDI and
insecure/host-network entitlements are disabled. The approved builder catalog is
created only after authenticated worker readiness succeeds. Revision 3 enables
the Direct Containerfile source described below. The milestone-2 acceptance ledger
is in `docs/milestone-2-acceptance.md`; the later acceptance reports cover
multi-node delivery/upgrades and optional exports.

### Direct Containerfile builds

Provision the default worker before creating a build. Its endpoint, pinned
BuildKit/frontend versions, isolation configuration and mTLS identity are owned
by the installation. Authors select `builderRef: default`; they do not supply an
engine address, Pod template or engine options.

For example, create a same-namespace ConfigMap named `hello-context` containing a
Linux/amd64 static executable under key `hello`, then declare:

```yaml
apiVersion: kimage.dev/v1alpha1
kind: Image
metadata:
  name: hello
spec:
  version: 1.0.0
  platform: {os: linux, architecture: amd64}
  source:
    build:
      format: Containerfile
      execution: {strategy: Direct, builderRef: default}
      cache: {mode: Local}
      definition:
        inline: |
          FROM scratch
          COPY hello /hello
          ENTRYPOINT ["/hello"]
      inputs:
        contextPath: /workspace/context
        volumes:
          - name: context
            configMap:
              name: hello-context
              items: [{key: hello, path: hello, mode: 365}]
        volumeMounts:
          - {name: context, mountPath: /workspace/context, readOnly: true}
```

The decimal mode `365` is octal `0555`. Binary ConfigMap values retain their exact
bytes. Mounts and item mappings are bounded and validated; overlapping paths,
traversal, arbitrary volume sources and writable input mounts are refused.

Use `definition.fromFile.path` instead of `inline` for a mounted definition.
ConfigMap definitions mount below `/workspace/inputs`; Secret definitions mount
below `/run/kimage-build-secrets`. Definition placement never changes the COPY/ADD
context root. Secret definition bytes and their fingerprints stay out of normal
status, release provenance and logs. The worker disables build history and normal
recipe-bearing daemon logging; client progress is bounded and discarded privately.
Definition literals can still enter OCI image history, worker cache or deliberate
build output. Supply credentials through secret mounts rather than recipe literals.

Build-secret bindings select files from declared Secret mounts, for example
`buildSecrets: [{id: token, sourceFile: /run/kimage-build-secrets/auth/token}]`.
The Containerfile may consume this with `RUN --mount=type=secret,id=token ...`.
Ordinary RUN bind mounts use the frozen context. Secret mounts are separate from
ordinary context hashing and are not copied into the context. A recipe can still
explicitly write a secret into its output; authors control their recipes.

PVC inputs use `persistentVolumeClaim: {claimName: source, readOnly: true}`, a
read-only mount with optional `subPath`, and `inputs.producerHandoff: true`.
Producers must stop writing before capture. The operator checks known writers,
PVC/PV identity and placement, then copies a bounded snapshot. This is a producer
handoff contract, not an atomic filesystem snapshot. ConfigMap/Secret selections
are copied to immutable, attempt-owned objects. Source changes during capture or
execution invalidate that attempt.

`Local` may reuse the dedicated worker's intermediates. `Disabled` passes the
real frontend's no-cache option. Secret **values** do not automatically invalidate
the build cache; use a non-sensitive `cache.invalidationToken` when a command's
output depends on changed secret values. The initial implementation conservatively
disables cache for solves carrying this token. Cache-mode/placement changes alone
retain an already published release. Changed output under an existing Image
UID/version is `VersionConflict`; request a new exact version to publish it.

This offline adapter supports `FROM scratch` and provisioned local inputs.
Remote bases, remote ADD sources, external syntax frontends, other formats,
strategies and builders are refused or fail with an actionable bounded condition.
RUN uses network none. No remote cache or general base-image provisioning is
implemented.

Submitted cancellation terminates the owned client and fences the dedicated
worker under its execution lease. This clears disposable worker cache. Image
deletion uses a finalizer to wait for cleanup; operator restart recovers durable
attempt state and fences an expired owner before starting a fresh attempt.
Retained publication and ordinary workload delivery remain independent of worker
cache and source-object retention.

The node runtime sees `127.0.0.1:<port>/<namespace>/<image-uid>@sha256:...` (system
components use reserved `system-<component>` repositories). The node proxy only
reads digests, authenticates as `reader`, verifies HTTPS upstream and forwards Range
and conditional read headers. Distribution binds Pod loopback behind the TLS gate.
The admin-managed baseline trusts eligible node administrators as readers; namespace
admission does not provide hostile-tenant image confidentiality on a shared node.

## Archive declarations and retained releases

The current Image source is `ociArchive` with a same-namespace `claimName`, relative
`path`, expected root descriptor, `producerHandoff: true`, and top-level
`platform: {os: linux, architecture: amd64}`. The producer must stop writes during
capture; a read-only consumer is not an atomic filesystem snapshot. Files must be
readable by UID 65532. The initializer only prepares an emptyDir snapshot; the
source is never mounted there or changed to make it readable.

Archive verification distinguishes root/index/config/layer/archive digests and
uncompressed DiffIDs. It preserves original graph bytes and media types. Expected
root equality makes retry of an already retained archive content-idempotent without
rereading its removed input. The first release's provenance remains immutable.
Different content for the same Image UID/version fails VersionConflict. Image
recreation gets a new UID/repository; Image deletion does not cascade releases.

Selectors are exact versions, explicit channels or semver ranges. Prefer the
readable colon form; the query form remains supported:

```text
platform://apps/service:1.2.4
platform://apps/service:stable
platform://apps/service:^1.2.0
platform://apps/service:~1.2.0
platform://apps/service?version=^1.2.0
```

For example, use this in a Pod or a Deployment's Pod template:

```yaml
containers:
  - name: service
    image: "platform://apps/service:^1.2.0"
```

Colon ranges start with an explicit operator (`^`, `~`, `>`, `<`, `=`) or `*`.
Numeric-leading colon selectors must be complete exact versions: `:1.2` and
`:1.2.*` are rejected. Compound ranges such as `:>=1.2.0, <2.0.0` work in quoted
YAML strings. Query ranges accept readable characters or percent encoding,
decoded once; encoding is unnecessary in these manifest examples. Colon ranges
use literal characters. Combining a colon selector with a query is rejected.

There is no implicit latest, version build metadata is rejected, and cross-namespace
consumption is denied. Admission leaves templates symbolic and pins actual CREATE
Pods, including init sidecars, to digest references with IfNotPresent. Channel/range
changes affect newly created Pods and can produce mixed versions; use exact versions
for uniform rollouts. Direct internal references are denied in application scope.
Metadata signatures bind one admission request, not tenant authorization.

### Workload and custom-controller compatibility

The same selector works in a Deployment, StatefulSet, DaemonSet, ReplicaSet,
ReplicationController, or Job at `spec.template.spec.containers[].image`, and in a
CronJob at `spec.jobTemplate.spec.template.spec.containers[].image`. The equivalent
`initContainers` image fields also work, including native sidecars with
`restartPolicy: Always`. Each controller-created Pod passes through the same
admission path. Parent templates retain selectors and their normal rollout policy.

Custom controllers can use their own image or Pod-template fields, provided their
CRD schema, validation, and reconciliation accept `platform://` and copy it
unchanged into a Pod image field. The Image and generated Pod must share the
configured application namespace; the controller can run elsewhere. Controllers
that parse OCI syntax or fetch registry metadata before Pod creation need an
adapter. There is no generic recursive replacement of arbitrary CRD fields.
ConfigMap strings, environment/argument values, Containerfile image references,
image-backed volumes, static Pods, and ephemeral-container updates are outside
the 0.2.0 selector contract.

CI exercises the real admission handler and catalog capture for seven built-in
controller template paths and a custom template path, normal/init/sidecar
containers, reinvocation, and replacement selection after a new version appears.
`scripts/live-workload-consumers.py` additionally exercises actual built-in
controllers and a small custom-controller fixture on the owned QEMU test cluster.
It requires that fixture's published build-probe image, records successful pulls
and starts, and removes its temporary workloads and CRD. It does not establish
compatibility with every third-party operator's validation or registry behavior.
The [recorded live results](workload-consumers-results.json) cover all eight cases
and complete cleanup. That retained fixture predates readable colon ranges, so
its live run uses `--selector-format legacy-query`; current-handler CI coverage
independently verifies both readable colon and encoded query forms.

The native control-plane phase provisions administrator-controlled system exclusion,
application namespace routing and API-authenticated source-reader exceptions. It
activates the fail-closed webhook only after schemas and serving endpoint are ready. With namespace-wide
routing, webhook outage can block unmanaged Pod creation too; selective failure
behavior is not claimed. No opt-in Pod label is a product consumption contract.

Build definitions, mounts/secrets, Direct/BuildKit and cache modes are described
in [the build design](build-design.md). See [the export design](export-design.md)
and [the milestone-3 report](milestone-3-acceptance.md) for the implemented export,
multi-node delivery, recovery and serving upgrade profiles.

## Recovery, backup, retention and permissions

Keep current and previous trusted bundles and issuer credentials independently of
running workloads. `recover` with the bootstrap arguments recreates carrier serving
when ordinary serving Pods are absent, reusing the same PVC. It cannot repair a
lost PVC, unusable node/control plane or missing carrier. Use `verify-remote` to
revalidate retained graph bytes without an original archive. The isolated VM passed cache-absent system recovery and a fresh uncached ordinary
pull; see the acceptance ledger for the exact prerequisite and network profile.

One RWO PVC/registry has capacity/attachment/topology limits and is not HA. Back up
the entire Distribution backend together with ImageRelease metadata; preserve UID,
root descriptors and schema coordination state. No automatic release/blob GC is
implemented. Alpha published releases have a permanent retention pin (an indefinite
grace period). The retention webhook denies their deletion and removal of that pin.
It allows the authenticated operator to discard obsolete, unverified candidate
metadata using UID/resourceVersion guards; it never deletes registry blobs. This
conservative policy avoids an admission/deletion reference-count race by retaining
every selectable release. Explicit artifact purge remains unsupported.

Uninstall removes webhooks before workloads. Retained finalizers still prevent
final metadata removal while admission is absent, but a DELETE can mark a release
terminating and unavailable for new selection. Administrator bypass of admission,
forced finalizer removal, namespace deletion and direct backend deletion are outside
the retention guarantee. Keep workload pins by preserving the release, metadata
and complete backend; do not force-remove finalizers as ordinary cleanup.

`uninstall` removes only the installation's labelled, name-scoped Pods, Services,
ConfigMaps and Secrets with UID/resourceVersion preconditions. It waits for Pods to
terminate and retains PVCs, CRDs, Images and ImageReleases. It never deletes remote
content. Namespace deletion and storage-provider loss are separate destructive
administrator actions.

The operator identity has namespaced Image/status/release access and
bounded input Pod/exec/PVC access, selected PV/node reads, built-in Lease/diagnostic
permissions and CRD management. Limit CRD GET/PATCH/UPDATE to the two known names;
top-level CREATE cannot be restricted by resourceNames. Admission can read
Images/releases and the bounded administrator-approved node/proxy readiness records.
It cannot publish or modify cluster objects. Registry/proxy/input processes need no Kubernetes token.
Do not grant them runtime-socket, CRD-management or publisher permissions. Native control-plane provisioning creates these separate identities and permissions.

Schema revision 4 refuses revision-1 PoC definitions, foreign spec field owners,
unsupported conversion and any unimplemented upgrade/downgrade. It checks the whole
definition set before coordination writes; no CRD deletion/force apply is used.
The exact shipped revision-2 and revision-3 schemas have supported additive
transitions to revision 4. The real fixture exercised 2 → 3 and 3 → 4 while
preserving CRD and release identities. Storage-version migration is unsupported.

For a revalidated access path, set both `KIMAGE_API_SERVER=https://<address>:<port>`
and `KIMAGE_TLS_SERVER_NAME=<certificate-name>`. Native clients preserve kubeconfig
CA/credentials, refuse insecure verification and bound connect/read timeouts.
Neither variable changes kubeconfig or node configuration.

### Control-plane installation

For a full installation, use an administrator-controlled system namespace with
`--namespace`, and pass a distinct `--application-namespace` for Images, their
source PVCs and consuming Pods. Supply registry issuer files with `--credentials`
and an independent admission issuer directory to `control-plane` with
`--admission-credentials`. The latter contains `ca.crt`, `tls.crt`, `tls.key` and
at least 32 bytes in `resolution-key`. The serving leaf must cover
`<installation>-admission.<system-namespace>.svc`. The CA private key remains
outside the cluster. The test issuer script is a seven-day fixture issuer, not a
production renewal controller.

Run `bootstrap`, `seed`, `handover`, `control-plane`, then `builder` against the same
trusted inventory, supplying the separate issuer directory for each phase.
The operator provisions and validates schemas before admission is registered;
existing foreign/incompatible CRDs cause refusal before control-plane writes.
The `control-plane` phase is resumable and idempotent in the tested fixture.
`handover` uses a read-only server and a single-writer transition. Publication
pauses during writer replacement; interrupted streams require client retry.

Operator CRD CREATE permission cannot be restricted by resourceNames; subsequent
CRD reads/updates/patches are limited to the two managed names, and there is no CRD
DELETE permission. Application reconciliation permissions are scoped to the
application namespace. Admission can read Images/releases and narrowly selected
node/proxy readiness; it cannot publish or change Kubernetes objects.

The bootstrap launcher processes select UID/GID 0 explicitly, drop all Linux
capabilities, prevent privilege escalation, mount no runtime sockets or host paths,
and keep the container image filesystem read-only. This prevents dependence on
the cached carrier's configured USER. The operator/admission and source reader run
as UID/GID 65532. The source reader's root init helper mounts only its disposable
snapshot workspace and never mounts or changes source PVC permissions.


## Issuer renewal

`renew-tls` uses the cluster arguments and `--admission-credentials`, without bundle
arguments. Supply new issuer leaves in the current private directories. Both full
identities are validated for key pairing, CA, service SAN, key usage and validity
before either Secret changes. The command preserves CA bytes, reader/publisher
passwords and the resolution key, patches only the owned TLS fields with UID and
resourceVersion guards, and checks actual served DER certificate fingerprints
through verified API forwarding. Registry and admission reload projected files on
new connections; invalid reloads fail closed. Resume a partial operation with the
same issued material. Keep these current files for later installation/recovery.

CA, password and resolution-key changes are deliberately separate from leaf
renewal. Their alpha procedure uses an explicit maintenance window: preserve the
backend PVC, release metadata and trusted bundles, uninstall disposable serving,
and perform API recovery with the new issuer/read/write material. Existing
workloads and uncached-start behavior follow the registry-downtime policy. The
builder's own mutual-TLS material uses worker reprovisioning; its cache is disposable.
This maintenance rotation path passed the recorded credential-rotation checks. The local
seven-day test issuer is a fixture tool; production issuer scheduling remains an
administrator responsibility and the CA private key stays outside Kubernetes.


### Bootstrap payload cleanup

After ordinary registry/proxy readiness, handover removes its temporary launcher
and executable-chunk ConfigMaps. Cleanup compares each immutable payload with the
trusted bundle, checks serving digest references and refuses while an active Pod
still references a payload. Deletions use UID/resourceVersion preconditions;
repeating cleanup resumes after an interrupted deletion. `kimage-install cleanup`
accepts the usual installation/bundle arguments to repeat this phase explicitly.
Recovery retains its ordinary checkpoints, credentials, registry PVC and releases;
carrier payloads are recreated from the independently retained bundle when needed.
The live fixture verified active-mount refusal, conflicting-byte preservation and
idempotent cleanup; see `docs/bootstrap-cleanup-results.json`.

## Additional nodes and serving image rollout

Use `join-node` with the original primary installation arguments, a trusted bundle
and `--target-node <additional-node>`. The target must meet the same pinned
Linux/containerd/cached-carrier prerequisites. It receives executable chunks
through the API, pulls its pinned proxy normally, switches to an ordinary proxy
Pod, and removes bootstrap payloads. Endpoint approval is recorded only after
ordinary serving readiness. The approval binds the Node UID, scope, proxy digest,
fixed credential volume and loopback/upstream arguments. Repeating the command
preserves an already-correct serving Pod. `--recover` permits replacement of an
owned stale proxy after a Node UID replacement; it never adopts a foreign port
owner. Retain the original administrator kubeconfig and trusted bundle for this
operation.

`upgrade-serving` takes the original installation arguments and target
`--bundle/--trusted-inventory`, plus independently trusted
`--previous-bundle/--previous-trusted-inventory`. This command updates only the
registry/gateway/proxy images. The other component roots and executable-version
compatibility window must match. It serializes serving transitions with a
renewable Lease, saves immutable replacement templates, stages verified graphs,
and warms replacement images on each explicitly approved node before changing
serving processes. A read-only registry mounts the same PVC during single-writer
handover. Proxies overlap after proving matching credentials/upstream scope. A
crashed installer can resume the same transition after its Lease expires; the
checkpoint can recreate a missing ordinary writer while read-only serving remains
available. If that path is also lost, use retained-bundle `recover` first.

Rollback is the same command with the independently trusted source and target
bundles reversed. The supported binary window is Distribution 3.1.1 ↔ 3.1.2 and
kimage gateway/proxy 0.1.0 ↔ 0.2.0. The filesystem configuration, data mount and
registry command must match the shipped profile; storage overrides are refused.
The bundle API revisions and every non-serving component version/root must match.
This command never changes CRDs or migrates persisted data. Other binary/storage
transitions require a separately implemented and verified compatibility path.
Native installer and bundle identities stay independent from the control plane.

Distribution 3.1.2 includes security fixes; prefer it for ordinary operation.
The older 3.1.1 bundle is a bounded compatibility/rollback option, not a preferred
installation. The test registry is not configured as an external pull-through
cache. See the upstream [3.1.2 release notes](https://github.com/distribution/distribution/releases/tag/v3.1.2).

The packer records `--registry-version` and `--serving-version`; their defaults
are 3.1.2 and 0.2.0. `--gateway` and `--proxy` can select independently retained
serving binaries while preserving the other component roots. Use trusted actual
release executables: changing a version string or an OCI label is not a binary
upgrade. Both bundles must include their independent native installers and pinned
builder dependencies for full API recovery.

The registry remains a single PVC-backed authority. Running application containers
continue during serving downtime. New uncached pulls wait/fail and are retried by
the normal kubelet path when serving returns; retained publication is not marked
unavailable merely because a serving process is down. Endpoint eligibility is an
admission-time snapshot, not continuous rescheduling of already-admitted Pods.
Warmup Pods and rollout checkpoints are bounded per requested bundle/node set and
are removed by ordinary uninstall; retained registry data and release metadata
remain separate from those disposable resources.

## Aggregate observability

Operator and admission expose `/metrics` on their existing health listeners. The
0.2.0 proxy exposes `/metrics` on its loopback listener and requires the same
canonical Host authority as registry reads. Use API port-forwarding for inspection;
the proxy does not expose diagnostics on another network interface. Output uses
fixed metric names and three fixed stage labels, never names, UIDs, digests,
repositories, input contents or credentials as labels.

`kimage_stage_duration_seconds_count/sum`, `kimage_stage_active`,
`kimage_stage_failures_total` and `kimage_stage_transferred_bytes_total` cover
build attempts, registry publication/readback, and proxy reads. Durations cover
the measured attempt or response stream, including cancellation/abandonment;
fixed-length HTTP completion is recognized at the declared byte count even when
the server does not poll EOF, while short/error bodies count as failures.
The duration metrics are process-local summaries without quantiles. Build bytes count OCI export
bytes received by the controller. Publication bytes count accepted uploads and
readback bytes, including reused content readback. Pull bytes count response
bytes observed in the proxy stream. These are observed transfer counters, not
unique durable storage usage or a promise of client consumption.

`kimage_reconcile_waiting` counts current waiters for the controller concurrency
permit. `kimage_images_pending` counts requested non-Ready Images, including
prerequisite failures/backoff; it is not a count of actively running solves.
`kimage_images_ready` and `kimage_releases_verified` report aggregate catalog
readiness. Catalog sampling uses pages of 100, at most 100 pages per resource,
a 30-second deadline and a ten-second interval. Incomplete samples do not replace
the previous complete counts. Check `kimage_catalog_sample_complete` and
`kimage_catalog_sample_timestamp_seconds` before interpreting those gauges.

`kimage_nodes_eligible` is admission's last successful endpoint-readiness sample;
check `kimage_node_sample_timestamp_seconds` for freshness. It does not continuously
reschedule existing Pods. Existing reconcile/admission counters retain their
fixed decision/error labels. Structured publication and transition logs carry
identities and operation/digest fields where useful, with sanitized reasons.

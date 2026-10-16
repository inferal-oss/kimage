# Direct build implementation decisions

For 0.3.0 platform selection, default on-demand production, eager configuration,
and namespace/cluster installation, see [native platforms and scope](platforms-and-scope.md).
Earlier acceptance receipts below describe the historical amd64 fixture; native
Kubernetes E2E now covers both architectures and both installation scopes.

This records the milestone-2 design and its acceptance boundaries. Revision 3 now
executes `source.build`; the first public API build and resolved workload passed
on the owned isolated fixture. The milestone-2 matrix passed; see `milestone-2-acceptance.md` for exact evidence
and limits. The revision-2 definitions are saved in
`crates/control/schemas/revision-2.json` for explicit compatibility checks before
extending the operator-owned schema. No CRD deletion or stored-version migration
is needed for an additive source variant.

## Boundaries

The API selects a typed format, an inline-or-file definition, named input volumes
and mounts, a strategy, an administrator-approved builder and a cache policy.
Containerfile syntax and BuildKit options belong in their adapters. The release
controller receives a verified OCI artifact and execution receipt through the
existing verifier/publication path. Neither admission nor retained-artifact
delivery depends on the builder.

Keep logical inputs separate from attempts and worker/cache placement. An already
published artifact remains usable without source objects, the worker or its cache.
Relevant changed inputs require a new logical identity; a different result for
the same Image UID/version is a conflict. A cache-mode or placement change alone
does not force a published version to rebuild.

## Frozen inputs

Read each referenced ConfigMap/Secret through the Kubernetes API once per selected
attempt, validate keys/binary values/modes, record UID/resourceVersion, and create
owned immutable selected snapshots. Secret copies retain Kubernetes Secret access
control and memory-backed projections; they are not ordinary context files.
This does not claim encryption of etcd unless the cluster provides it.

Use a credential-free reader to assemble ordinary ConfigMap/PVC files beneath the
approved context root. Copy a bounded UTF-8 inline or mounted definition into a
separate reserved frontend directory. Its original mount location never changes
COPY/ADD context paths. Secret mounts live under a separate memory-backed root and
are exposed only by explicit BuildKit secret IDs. Reject traversal, unsupported
volumes, symlink escapes, overlapping mounts, writable sources and control-directory
replacement. Validate subPath through the same mounted-input resolver.

Retain the existing PVC binding checks and known-writer refusal. Producer handoff
is a contract, not an atomic filesystem snapshot. Check claim/volume identities
again after capture. Source mutations during capture/solve must invalidate stale
acknowledgments; a status-watch event must still honor durable retry deadlines.

Ordinary content has a public bounded digest. Secret definition bytes and their
sensitive fingerprints remain in private attempt state, never normal status,
logs or release provenance. Use an opaque attempt identifier to distinguish such
snapshots. Secret *values* do not implicitly invalidate BuildKit cache; an explicit
non-sensitive invalidation token is part of the logical request when needed.

## Execution and cancellation

Use the provisioned BuildKit 0.33.0 worker and built-in dockerfile.v0 frontend
1.27.0, authenticated with its dedicated mTLS client identity. Invoke pinned
buildctl using argument arrays in a bounded non-root client/session process.
Capture recipe-bearing progress in bounded private attempt logs rather than
forwarding it to Pod logs. The default daemon log configuration must also prevent ordinary recipe
errors from leaking Secret-sourced definitions before that feature is enabled.

The pinned [buildctl source](https://github.com/moby/buildkit/blob/v0.33.0/cmd/buildctl/build.go)
supports local context/definition sessions, explicit secret mappings, OCI export,
and no-cache. Its solve reference is generated internally, and `--ref-file` writes
that reference at command exit. Do not invent an early-reference or cancel CLI.
`debug ctl` pins/unpins/deletes records; it is not a cancellation API.

The initial strategy can fence cancellation by terminating the owned client and
replacing the exact dedicated worker Pod, waiting for the old UID to disappear.
Serialize this with the single approved worker's execution lease. This deliberately
discards its disposable cache and requires a live cancellation test; dropping an
API exec stream alone is insufficient. On restart, either adopt a complete owned
export or fence the incomplete attempt before retrying its frozen input snapshot.

Local may reuse that worker's intermediates; Disabled uses the real frontend's
no-cache option. Keep output and active workspace bounds separate from the worker
GC budget and kubelet eviction limits. No remote cache is implied.

## Offline and acceptance

Worker egress is denied, insecure and host-network entitlements stay disabled,
and the default fixture remains disconnected at its hypervisor. RUN network none
is only one part of this restriction. Do not enable user-selected external
frontends, unchecked engine options or credentials for arbitrary source pulls.
Start with scratch plus provisioned regular inputs and a real executable RUN.

The required real matrix includes inline, ConfigMap/PVC/subPath/Secret definitions,
context-root independence, text/binary/key/mode mapping, frozen object mutation,
RUN bind/secret mounts, Local/Disabled and changed-input behavior, version conflict,
failure/cancellation/restart, retention after cache loss, and a resolved running Pod.
Negative declarations must fail before launching execution where possible. No
second production format, strategy or distributed scheduler is required.

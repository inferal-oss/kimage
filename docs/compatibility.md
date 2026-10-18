# Upgrade and rollback compatibility

Version 0.6.1 establishes the reader baseline for future additive API changes.
Its CRD definitions remain exactly schema revision 6, as shipped in 0.6.0.
The declared 0.6.0 to 0.6.1 transition supports a reverse transition using the
0.6.1 coordinator, subject to preflight checks. Registry storage, installation
state, build state and operation journals keep their existing formats.
The one-way 0.5.2 to 0.6.0 boundary remains unchanged.

## Binary rollback keeps the newer schema

A schema revision is the minimum definition a binary understands, rather than
an instruction to replace every installed CRD with that exact revision.
From the 0.6.1 reader baseline, a newer CRD may remain installed after rollback
when its declared reader contract and structural proof pass. The schema manager
leaves its definition, revision, ownership, UIDs and stored data intact.
The schema diagnostic reports the observed shared revision separately from the
running reader's revision and version.

The `kimage.dev/schema-readers` annotation declares format 1, baseline revision 6,
minimum reader 0.6.1, policy `additive-optional-v1`, and a canonical definition
digest. Matching annotation values alone are insufficient: the reader checks
schema ownership, field ownership, definition integrity and the actual changes.
Known fields, defaults, required lists, validation rules, storage versions and
conversion settings cannot change. Optional new fields cannot introduce defaults
that activate on existing objects. Enum widening is permitted only in prepared
opaque areas. Stable identity, OCI descriptor, placement and demand fields keep
their existing contracts.

Prepared extension areas are new Image spec fields, source fields, new status
fields, opaque build/operation state, and new release/provenance fields. Other
changes require an earlier reader release or an explicit versioned migration.
This foundation does not promise arbitrary future storage or serving protocols.

## Unsupported builds are isolated

An Image can declare required capabilities in metadata:

```yaml
metadata:
  annotations:
    kimage.dev/required-capabilities: '["frozen-oci-inputs-v1"]'
```

The initial capabilities are `oci-archive-v1`, `containerfile-v1` and
`frozen-oci-inputs-v1`. A future writer must declare capabilities for changed
execution semantics and copy requirements to its native variants.

An unknown capability, source, spec extension or execution-state format pauses
only that Image with a `BuildSupported=False` condition. The controller preserves
opaque data and existing published readiness. It does not reinterpret the new
feature as an older build. Native children also check their parent's requirements.
Other Images continue building; admission can still resolve verified published
roots using the stable release identity and OCI descriptor.

Status updates compare and patch owned fields, avoiding repeated writes for unrelated metadata. Unknown status data stays stored, and newer
provenance remains opaque when its execution receipt cannot be decoded. Do not
remove capability declarations merely to force an older controller to build.
Finish or remove unsupported active operations with a compatible newer reader.

## The previous release window

The release gate exercises the actual published predecessor, CLI and Helm,
both scopes and native architectures, plus real Argo chart transitions. Use the
newer CLI to coordinate upgrades and rollbacks. Preflight remains read-only;
operation leases and journals protect interruptions and resumable application.

Published 0.6.0 was not prepared to read newer schemas or fence unknown features.
Returning to it therefore requires exact schema revision 6 and no unsupported
resources in the selected installation's scope. These checks run before mutation.
A later additive-schema upgrade whose minimum reader is 0.6.1 can return to
0.6.1 while retaining the newer schema and pausing affected builds.

The disposable future-version acceptance fixture uses real independently named
binaries, schema revision 7, a populated optional Image field and additional
release provenance. It must verify newer-data retention, published native pulls,
unrelated builds, interruption recovery and schema non-replacement after returning
to the unchanged baseline binary. It is a conformance fixture, not a published
0.6.2 release. Real mixed-cluster acceptance keeps shared schema revision 6 while
testing the published predecessor round trips and capability fencing; synthetic
future CRDs belong in disposable CI clusters.

Each actual release declares and tests its exact supported edges. A genuinely
breaking change needs an explicit migration and rollback policy before upgrade.

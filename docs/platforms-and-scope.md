# Native platforms, build policy, and installation scope

kimage 0.3.0 produces and verifies `linux/amd64` and `linux/arm64` images on
native workers. An Image without `spec.platform` or `spec.platforms` uses automatic
selection. The default build policy is **OnDemand**.

## What selects the platform?

Admission evaluates the actual Pod's `nodeName`, `nodeSelector`, and required
node affinity against eligible, Ready nodes with approved local proxies.
Deployment, Job, CronJob, and custom-controller templates use the same path when
their controller creates a Pod. Preferred affinity is a preference, so it does
not narrow the required platform set.

| Placement and Image declaration | Build and consumption |
| --- | --- |
| Placement selects only amd64 | Build/use the amd64 variant |
| Placement selects only arm64 | Build/use the arm64 variant |
| Placement leaves both architectures eligible | Build both and publish a verified OCI index; kubelet selects its native child |
| No placement, only one native architecture available | Build/use that one variant |
| No consumer yet, automatic Image, OnDemand policy | Remain `AwaitingDemand`; no build starts |
| No consumer yet, automatic Image, Eager policy | Build every available supported native platform |
| Explicit `spec.platform` or `spec.platforms` | Request those native variants directly, limited to available platforms |
| Unsupported or contradictory placement | Reject; do not guess another architecture |

All managed image references in one Pod contribute their platform constraints.
Admission restricts scheduling to eligible nodes that can run every selected
image. It preserves required user placement constraints and pins a native
manifest or complete index digest. A later node addition does not change an
already admitted Pod.

For example, this workload requests arm64 even on a mixed cluster:

```yaml
spec:
  template:
    spec:
      nodeSelector:
        kubernetes.io/arch: arm64
      containers:
        - name: service
          image: "platform://apps/service:^1.2.0"
```

An Image can explicitly request one platform:

```yaml
spec:
  platform: {os: linux, architecture: arm64}
```

Or a set, with `platform` omitted:

```yaml
spec:
  platforms:
    - {os: linux, architecture: amd64}
    - {os: linux, architecture: arm64}
```

Do not specify both forms. A version is immutable per Image UID and platform.
Native variants have independent release identities; their multi-platform index
has a separate identity. Adding an arm64 variant cannot replace an existing
amd64 release. `status.platformReleases` lists the native release names;
`status.releaseRef` points at the available native release or aggregate index.

## On-demand and eager production

The administrator-owned ConfigMap `<installation>-config` lives in the system
namespace. Its `data.configuration.json` value defaults to:

```json
{"schema":1,"buildPolicy":"OnDemand"}
```

Use `"Eager"` to prepare automatic Images before workloads exist. Edit this value
with `kubectl edit configmap dev-config -n kimage-system`. The operator watches
configuration changes; rerunning installation preserves this administrator policy.
Invalid policy values fail closed.

A real Pod CREATE with no matching ready release records bounded platform demand
in an `ImageDemand` in the system namespace, then returns `ImageBuildPending`.
Normal workload controllers retry Pod creation while the operator builds.
A direct `kubectl apply` of a Pod can return an error: wait for the Image to become
Ready and retry it. Server dry-run creates no demand and starts no build.
Apply both the Image and its Deployment before waiting for default automatic
production. Explicit platforms or Eager policy can prebuild before a consumer.

Demand is bound to the Image UID, generation, and declared current version.
A selector cannot request arbitrary historical builds. Within a generation,
demands accumulate: an amd64 consumer first can prepare one variant, and a later
arm64 or unconstrained consumer can add the other. Different input captures across
variants are rejected with `PlatformSourceSnapshotMismatch`; keep inputs stable
and use a new version for changed content.

## Native workers and portable inputs

The standard installer chooses the primary node's architecture from Kubernetes,
fetches its verified bundle, and discovers one Ready schedulable native worker
node for each other supported architecture. It fetches the additional bundle,
seeds verified native components through the registry API, joins that node's
local proxy, and registers its authenticated worker. Bundle selection follows
the cluster node, even when the administrator uses a different laptop platform.
Extra nodes of an already registered architecture can use `join-node`.

There is no automatic QEMU build emulation. An unavailable architecture remains
pending or is rejected when placement cannot be satisfied. New architectures
added after installation need their matching bundle, node join, and worker setup.
With local `--bundle` input on a mixed cluster, provision each architecture through
the explicit `join-node` and `builder` phases; automatic official fetching is the
simple mixed-cluster installation path. `--offline` requires cached bytes for
every selected architecture.

Architecture-specific executables remain your inputs. A portable Containerfile
can use the standard BuildKit platform argument:

```dockerfile
FROM scratch
ARG TARGETARCH
COPY hello-${TARGETARCH} /hello
ENTRYPOINT ["/hello"]
```

Provide `hello-amd64` and `hello-arm64` if both variants can be requested. For one
native platform, supply its matching file. kimage selects the worker and validates
the output OCI platform; it does not translate an executable copied from inputs.

## Namespace or cluster-wide installation

Namespace scope is the default. Add `--application-namespace apps` to manage that
one application namespace. Both modes use a dedicated system namespace.

```sh
kimage install --scope namespace \
  --namespace kimage-system --application-namespace apps \
  <the remaining cluster and credential arguments>
```

For all ordinary application namespaces, including namespaces created later:

```sh
kimage install --scope cluster \
  --namespace kimage-system --exclude-namespace shared-infrastructure \
  <the remaining cluster and credential arguments>
```

Omit `--application-namespace` in cluster mode. The installation always excludes
its system namespace and `kube-system`, `kube-public`, and `kube-node-lease` from
application admission and reconciliation. Repeat `--exclude-namespace` as needed.
Overlapping installations are refused. Scope is part of the retained installation
configuration; do not change it by rerunning with different flags.

Images, inputs, export credentials, selectors, and releases remain namespaced.
A Pod in `apps-a` cannot consume `platform://apps-b/service:1.0.0`, including in a
cluster-wide installation. Two namespaces can each define their own `service`
Image. The optional exporter follows the configured scope and uses each Image's
own namespace for destination credentials.

Both modes require administrator setup for shared CRDs, webhook configuration,
and RBAC. Namespace scope limits application permissions to its selected
namespace; it is not an unprivileged tenant installation. Cluster scope grants
broader application read/write permissions and applies runtime namespace filters.
Registry node administrators are trusted readers; this is not hostile-tenant
registry isolation. Uninstall keeps PVCs, CRDs, Images, and retained releases.

## Tests and release gate

`cargo test --workspace` covers platform placement and intersections, complete
multi-platform graph verification, immutable variant identities, default/eager
policy, demand generations, and controller API build creation. The
[Native Kubernetes E2E workflow](../.github/workflows/e2e.yml) runs fresh installs
for both scopes on native amd64 and arm64 Linux runners. It exercises real
builds/pulls, all supported controller templates, dry-run, pending demand, eager
production, RBAC and namespace boundaries, and retained storage on uninstall.
The release workflow requires that matrix to pass before publication.


The [opt-in mixed-cluster E2E](mixed-cluster-e2e.md) separately exercises both
architectures in one real cluster, using fresh Images for unknown and explicit
amd64/arm64 workload placement. It checks exact native builds and actual pulls
of the selected index or manifest. It is ignored by default and explicitly
configured for an administrator-approved mixed cluster.

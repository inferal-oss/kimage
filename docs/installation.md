# Installation

Starting with 0.4.0, kimage components are ordinary multi-platform images in
`ghcr.io/inferal-oss/kimage`. GitHub Actions builds native amd64 and arm64 images
on release. The installer and Helm chart use the same immutable OCI index digests;
kubelet chooses the node's architecture. No component executable or image layer
is uploaded through Kubernetes ConfigMaps, port-forwarding, or an installer Pod.

The included PVC-backed registry still stores **application images** built by
kimage. GHCR supplies **kimage's own components**. Application authors continue to
request `platform://apps/service:^1.2.0` without choosing a publishing registry.

## Prerequisites

Use administrator kubeconfig, Ready Linux amd64/arm64 nodes, persistent storage,
and the supported user-namespace/runtime profile. The default `inferal` profile
checks RKE2 `v1.36.5+rke2r1` and containerd `2.3.4-k3s1`. Select
`--runtime-profile k3s` for the tested K3s `v1.36.5+k3s1` / containerd
`2.3.4-k3s1.36` profile. Helm uses `runtimeProfile: k3s` for the same selection.
Other exact runtime versions are refused. This remains alpha software.

The native BuildKit worker needs Kubernetes Pod user namespaces, idmapped volumes,
the documented nested-namespace capabilities, CNI policy enforcement and suitable
storage. A generic kind or k3d cluster is not automatically supported. Workers
have no host filesystem or container-runtime socket mounts. Use a dedicated
system namespace whose policies permit these administrator-managed components.
Both installation scopes require administrator access for shared CRDs and webhooks.

Nodes must be able to pull the public component images over HTTPS. The installer
needs HTTPS access to GitHub release metadata and the small catalog asset. There
is no required carrier image, credential-generation script, source checkout, or
preloaded kimage component. A default StorageClass is used unless overridden.

## Install with the CLI

Install Rust 1.99.0 or newer, then:

```sh
cargo install kimage-control --version 0.5.1 --locked --bin kimage --bin kimage-install
kimage install --application-namespace apps
```

`kimage-install` is a compatible alias. Prebuilt Linux/macOS amd64/arm64 installers
are available in [GitHub releases](https://github.com/inferal-oss/kimage/releases).
The Cargo crate named `kimage` belongs to a separate project.

With no flags, `kimage install` manages the current kubeconfig namespace, uses
`kimage-system` for system components, installation identity `kimage`, port 17701,
a 1Gi retained PVC, and the default StorageClass. It creates missing requested
namespaces. The system namespace must differ from the application namespace.

For the tested K3s profile and an explicit StorageClass:

```sh
kimage install --application-namespace apps \
  --runtime-profile k3s --storage-class local-path
```

To manage all ordinary namespaces, including future namespaces:

```sh
kimage install --scope cluster --exclude-namespace infrastructure
```

Repeat `--exclude-namespace` as needed. The system namespace and Kubernetes system
namespaces are always excluded. Namespace scope restricts application RBAC to one
namespace. Overlapping kimage scopes are refused. Images and their consumers must
still be in the same application namespace in either mode.

Optional export can be enabled at installation with repeated
`--export-origin https://registry.example/` flags. No export origin is approved by
default.

The CLI fetches the exact release's bounded, SHA-256-verified `component-images.json`.
It generates separate registry, admission and worker TLS identities and independent
read/publish credentials in process. CA private keys are not deployed. It applies
small ordinary resources, waits for components and authenticated worker readiness,
then enables admission. Reapply the same command after an interruption. It reuses
credentials and preserves the administrator's OnDemand/Eager ConfigMap policy.
Foreign resources and a different installation manager are refused.

## Install with Helm

The chart repository is hosted alongside the website:

```sh
helm repo add kimage https://kimage.dev/charts
helm repo update
kubectl create namespace apps
helm install kimage kimage/kimage --version 0.5.1 \
  --namespace kimage-system --create-namespace \
  --set applicationNamespace=apps --wait --wait-for-jobs
```

Use Helm 3.14 or newer. The chart queries live Ready Nodes during install/upgrade;
`helm template` offline requires explicit full Node objects in `nodes`. Helm does
not create the application namespace. Add `--set runtimeProfile=k3s` for the tested
K3s profile, or `--set persistence.storageClass=local-path` when needed.

For cluster scope, omit `applicationNamespace` and set `--set scope=cluster`.
Use a values file for `excludedNamespaces`. Important values are:

| Value | Default and purpose |
| --- | --- |
| `scope` | `namespace` or `cluster` |
| `applicationNamespace` | `default` in namespace scope |
| `excludedNamespaces` | Extra cluster-scope exclusions |
| `runtimeProfile` | `inferal`; `k3s` selects the tested K3s profile |
| `registryNode` | First eligible node on initial install; retained on reapply |
| `persistence.storageClass` | Cluster default |
| `persistence.size` | `1Gi` |
| `persistence.existingClaim` | Optional existing Bound claim |
| `buildPolicy` | Empty preserves the existing setting, otherwise defaults to `OnDemand`; accepts `Eager` |
| `images.repository` | Official GHCR repository, or an administrator-controlled mirror |
| `images.digests` | All eight multi-platform pins, populated in the published chart |
| `exporter.approvedOrigins` | Empty disables optional export; explicit HTTPS origins enable its separate identity |
| `workerNodes` | Optional architecture-to-node placement overrides |
| `port` | `17701`; use a distinct port for installations sharing nodes |

The published chart contains the release's image pins. The repository's source
chart intentionally has empty pins until release packaging verifies both native
image graphs. The operator manages the shared CRDs with guarded schema compatibility
checks; Helm does not own or delete them.

## Install with Argo CD

Use `gitops.enabled: true` with the published chart. Rendering is deterministic
and independent of live Kubernetes data. See the [Argo CD guide](argocd.md) for
copyable Applications, in-cluster credentials, readiness, upgrades and retention.

## Nodes, storage and reapplication

One native worker is registered per available architecture; every eligible node
gets a local read proxy. Component images are full amd64/arm64 indexes. Application
builds still follow [consumer placement and build policy](platforms-and-scope.md).
There is no QEMU build emulation.

The operator now reconciles node membership automatically. New eligible nodes
receive UID-bound read proxies; a newly available architecture receives a native
worker. Existing automatic workers stay in place while eligible. Lost membership
revokes serving approval; replacing a worker acquires its build Lease, terminates
old clients and waits for worker deletion before creating a replacement. A
partitioned worker can leave fencing pending. Do not force-delete it to make an
upgrade proceed.

The optional `<installation>-node-policy` ConfigMap contains `policy.json`:

```json
{"schema":1,"nodeSelector":{"kimage.dev/pool":"images"},"workerPins":{"arm64":"arm-worker"}}
```

It must carry label `kimage.dev/installation: <installation>`. An empty selector
includes every eligible node. Pins are administrator placement requirements; a
missing pinned node is reported instead of silently changing placement. Helm's
`nodePolicy` value sets this policy; null preserves administrator edits, and a
fresh install adopts explicit `workerNodes` values as pins. Ensure the retained
registry node matches the selector. Its name, UID and PVC never move automatically.
Use unique identities and ports for multiple installations. Scope, storage and
installation identity remain fixed during release transitions.

System registry/operator/admission use Deployments. Native worker and exact-node
proxy Pods preserve the existing node-identity and worker-recovery contracts.
The builder config is pinned and worker egress remains denied. Components can
restart by pulling their published images independently of the included registry.
Running applications survive registry downtime; new uncached application pulls
wait or fail until it returns. This is not an HA deployment.

## Mirrors and disconnected clusters

Mirror the eight complete OCI indexes, including **both** native manifests and
layers, using registry tooling that preserves digests. Update only `repository`
in an independently verified copy of `component-images.json`, then pass
`kimage install --image-catalog /path/component-images.json`. The file is an
explicit administrator trust input. Its release, roles, schema, digests and native
platform coverage are validated. For Helm, set `images.repository` to the mirror
and retain the published `images.digests`.

Node runtimes must already trust and reach the mirror using their normal registry
configuration. kimage does not alter node trust stores or transfer images through
the API. A disconnected cluster needs mirrored or already cached component images
and provisioned application inputs. Saving the small catalog alone is insufficient.
Public catalog downloads use no token; optional `KIMAGE_GITHUB_TOKEN` authenticates
only fixed GitHub metadata requests and is never forwarded to asset downloads.

## Upgrades, rollback and removal

Install the new CLI first. Updating the Cargo package does not change cluster
components. The current supported release edge is **0.5.0 to 0.5.1 and back**:

```sh
kimage upgrade --to 0.5.1 --check
kimage upgrade --to 0.5.1
kimage status --json
kimage doctor --json
kimage rollback --to 0.5.0 --check
kimage rollback --to 0.5.0
```

Add `--namespace` and `--installation` for a non-default system identity. Checks
are read-only. Actual transitions acquire a renewable installation Lease and
persist exact source/target catalogs and phases. They stop new reconciliation,
terminate old clients and workers before replacing component policy, preserve
credentials and administrator build policy, then verify component readiness and
retained PVC/PV/CRD/credential identity. Registry writes, managed Pod admission
and new uncached pulls can pause during the transition; running application
containers remain under their workload controller.

If interrupted, rerun the same exact command and catalog. A crashed process's
Lease expires after 30 seconds. Do not delete the journal or switch targets while
an operation is pending. The operation is marked Complete only after verification;
a failed transition does not silently roll back. `status` reports its phase and
`doctor` checks drift, native readiness and certificate validity, returning a
nonzero exit status when attention is required.
`NodeReconcileForbidden` points to missing operator permissions: compare its
system namespace Role and RoleBinding with the exact release's manifests.

For a Helm-managed installation, the same commands invoke Helm and preserve Helm
ownership and revision history. Keep a compatible `helm` executable on PATH.
Helm 3 and 4 are supported. Guarded transitions select ordinary three-way updates
on Helm 4 to coordinate with operator quiescing and reconciled node resources.
Upgrades use the exact release chart and explicit target image pins; rollback renders the exact older chart against current node placement as a new
Helm revision. New charts reject an unguarded
cross-release upgrade. Use the guarded commands for compatibility checks and
interrupt recovery; an old 0.4.1 chart cannot add guards to raw `helm rollback`.
Review history with `helm history kimage --namespace kimage-system`.
Preflight also checks that stable Helm history can recover an interrupted
revision without restoring stale credentials or build-policy bytes. If it reports
`HelmRecoveryCredentialDrift` or `HelmRecoveryPolicyDrift`, reapply the current
version's chart first to record the live values, then repeat the upgrade check.

The release's `release-compatibility.json` records supported exact edges and the
API, installation/build/journal, serving and registry storage formats. Unknown
edges are refused. Rollback to 0.4.1 requires an empty node selector because that
operator cannot enforce the newer selection policy. Worker pins and administrator
build policy are preserved. Restore default node selection before downgrading.

Only the tested compatibility edge is allowed. There is no implicit latest,
skipped release, shared CRD downgrade, storage migration or automatic adoption of
0.3.x bootstrap installations. Keep original installers and trusted bundles for
[legacy recovery](operations.md). `kimage install` remains a same-release
installation/resume command and refuses implicit release changes.

```sh
kimage uninstall --namespace kimage-system --installation kimage
# Or, for Helm-managed installations:
helm uninstall kimage --namespace kimage-system --wait
```

After rollback to a predecessor such as 0.5.0 or 0.4.1, stop its operator before
Helm removal to prevent worker reconciliation from racing chart deletion.
Version 0.4.1 can recreate workers without Helm tracking metadata. Stop the
operator, then remove its named workers before removing the chart:

```sh
kubectl -n kimage-system scale deployment/kimage-operator --replicas=0
kubectl -n kimage-system wait --for=delete pod -l kimage.dev/installation=kimage,kimage.dev/role=operator --timeout=180s
kubectl -n kimage-system delete pod kimage-builder-amd64 kimage-builder-arm64 --ignore-not-found --wait=true
helm uninstall kimage --namespace kimage-system --wait
```

Use your installation name and system namespace. The CLI uninstall command
already stops the operator before removing workers. Native acceptance tests fresh
0.5.1 Helm removal directly and uses this shutdown procedure after rollback to
the published predecessor.

Both preserve the registry PVC, Images, ImageReleases and shared CRDs. Do not delete
retained namespaces, PVCs, finalizers or schema coordination records as ordinary
cleanup. Back up the entire registry backend together with release metadata,
UIDs and schema state. Generated leaf certificates last ten years. Reapplication
preserves them; it does not implement automatic certificate renewal or CA rotation.

Version 0.4.1 fixes the intermittent `CleanupIdentityConflict` during CLI
uninstall when Pod status changes between LIST and DELETE. UID, specification
and ownership checks remain enforced. For an existing 0.4.0 installation, update
the CLI with the Cargo command above, then run the same uninstall command; no
component reinstallation or data migration is needed for this cleanup fix.

## Acceptance evidence

All eight [recorded native cases](native-install-results.json) passed for 0.4.0:
CLI and Helm, namespace and cluster scope, on amd64 and arm64. Each case starts
with an empty kimage runtime cache and pulls the complete component indexes from
a verified TLS registry. The checks cover authenticated worker readiness, actual
application builds/pulls, workload controllers, demand/eager policy, scope/RBAC,
overlap refusal, reapplication preserving credentials/policy/storage, and retained
artifacts after removal. Source and package checks also pass on Linux amd64/arm64
and macOS arm64.

The published CLI and public Helm chart also passed the
[real mixed-cluster acceptance](mixed-install-results.json): fresh Images for
unknown, amd64-only and arm64-only placement, ordinary native GHCR component
pulls, and uninstall/reinstall with complete retained OCI graph readback. The
receipt records the published CLI's retry and the subsequent cleanup fix.

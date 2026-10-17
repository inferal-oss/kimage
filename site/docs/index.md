# From an image declaration to a running workload.

A practical guide to producing, retaining, and delivering images inside Kubernetes. Start with the CLI and a small example; use the reference sections when you need them.

**Alpha, with a specific supported profile.** The CLI pulls pinned native component images from GHCR. Cluster setup needs administrator access, persistent storage and the runtime prerequisites below. The CLI generates credentials; Helm is also available.

## The idea

Declare the image you want as an `Image`, including its exact version and source. kimage manages build attempts, verifies the complete OCI graph, publishes it to a retained cluster-local registry, and records an immutable `ImageRelease`. Application authors do not create a separate build-run resource or configure an output registry for each image.

```
Image + provisioned inputs
  → build → verify → retained registry + ImageRelease
  → Pod admission selects a release → kubelet pulls its digest
```

Use kimage when image inputs already live in Kubernetes, when local image availability matters, or when the cluster must build and pull disconnected after dependencies have been provisioned. The current adapter builds scratch images from local inputs. Choose another workflow if you need arbitrary remote bases or a general CI pipeline today.

A release belongs to an Image UID and exact version. Once published, different content cannot replace it. Deleting inputs, the Image, or disposable builder cache does not delete its retained artifact.

## Install the CLI

Use Rust 1.99.0 or newer. No source checkout is needed:

```
cargo install kimage-control --version 0.5.2 --locked --bin kimage --bin kimage-install
kimage install --application-namespace apps
```

`kimage-install` is a compatible alias. The crate named `kimage` belongs to another project. Prebuilt Linux and macOS installers for amd64 and arm64 are available in [GitHub releases](https://github.com/inferal-oss/kimage/releases).

With no flags, `kimage install` manages the current kubeconfig namespace, uses `kimage-system` for components, a 1Gi retained PVC and the cluster's default StorageClass. It discovers native workers, generates credentials and waits for authenticated readiness. Use administrator kubeconfig and the [supported runtime](https://kimage.dev/docs/#compatibility). For the tested K3s profile, add `--runtime-profile k3s`.

Server binaries are also available from `kimage-control` if needed. Gateway and proxy retain serving protocol version 0.3.0 independently of package version 0.5.2.

## Release images and mirrors

Starting with 0.4.0, all eight system components are ordinary public images under `ghcr.io/inferal-oss/kimage`. GitHub Actions builds native amd64 and arm64 images on release, verifies their complete OCI graphs, and publishes immutable multi-platform indexes. Kubelet selects its node's native manifest.

The CLI fetches only a small `component-images.json` asset from its exact GitHub release, never `latest`. GitHub asset SHA-256 and size are verified, and the catalog must contain the expected roles, versions, schema and both native platforms. The Helm release chart contains the same image pins. No component executable or image layer is uploaded through Kubernetes ConfigMaps, installer Pods or port-forwarding.

GHCR supplies kimage's components. The included cluster-local registry still retains the application images that kimage builds. Workloads keep using `platform://` selectors.

| Option | Purpose |
| --- | --- |
| `--release 0.5.2` | Exact official release; defaults to the CLI package version. |
| `--image-catalog <path>` | Explicit administrator-trusted local catalog, including a registry mirror. |
| `images.repository` | Helm registry mirror override; retain all published digest pins. |

For disconnected installation, mirror all eight complete indexes with both architectures and preserve their digests. Change only the repository in an independently verified catalog, or set Helm's repository override. Nodes must already trust and reach that mirror using their normal runtime configuration. kimage does not edit node trust stores or transfer images through the API. A saved catalog alone is insufficient.

Public downloads need no token. Optional `KIMAGE_GITHUB_TOKEN` applies only to fixed GitHub metadata requests, never asset downloads. Bootstrap bundles and low-level recovery commands remain a legacy path for independently retained older installations.

## Your first image

**Before starting:** an administrator has completed [cluster setup](https://kimage.dev/docs/#cluster-setup), configured application namespace `apps`, and provisioned `builderRef: default`. Your kubectl context points to that development cluster.

### 1. Get the small example

Download just the three files. You can also open the [program](https://kimage.dev/examples/hello/main.rs), [Image](https://kimage.dev/examples/hello/image.yaml), and [Deployment](https://kimage.dev/examples/hello/deployment.yaml) directly.

```
mkdir -p examples/hello
for file in main.rs image.yaml deployment.yaml; do
  curl --fail --location --output "examples/hello/$file" \
    "https://kimage.dev/examples/hello/$file"
done
```

### 2. Compile a Linux executable

On a native Linux/amd64 or Linux/arm64 machine with Rust and a musl linker installed (`musl-tools` on Debian/Ubuntu):

```
case "$(uname -m)" in
  x86_64) ARCH=amd64; TARGET=x86_64-unknown-linux-musl ;;
  aarch64) ARCH=arm64; TARGET=aarch64-unknown-linux-musl ;;
  *) echo "Use a supported Linux build machine"; exit 1 ;;
esac
rustup target add "$TARGET"
mkdir -p .local/tutorial
rustc --edition=2024 --target "$TARGET" \
  -C opt-level=z -C strip=symbols -C panic=abort \
  examples/hello/main.rs -o ".local/tutorial/hello-$ARCH"
```

On macOS, use a matching native Linux build machine or container for this step. A native macOS executable cannot run inside the scratch image. On a mixed cluster, put both `hello-amd64` and `hello-arm64` in the input directory, compiling on matching native Linux build machines. One platform needs only its own file.

### 3. Provide input and request the image

```
kubectl -n apps create configmap hello-context \
  --from-file=.local/tutorial \
  --dry-run=client -o yaml | kubectl apply -f -
kubectl apply -f examples/hello/image.yaml
```

The example copies the executable into `FROM scratch` and requests version `1.0.0`. The controller handles execution and local publication. ConfigMaps suit small inputs; use a PVC for larger content. The automatic Image stays `AwaitingDemand` until its Deployment requests the needed native platform. Apply the Deployment before waiting.

### 4. Run it through a selector

```
kubectl apply -f examples/hello/deployment.yaml
kubectl -n apps wait --for=condition=Ready image/hello --timeout=10m
kubectl -n apps get imagereleases
kubectl -n apps rollout status deployment/hello --timeout=5m
kubectl -n apps logs deployment/hello
kubectl -n apps get pods -l app=hello \
  -o jsonpath='{.items[0].spec.containers[0].image}{"\n"}'
```

The Deployment keeps `"platform://apps/hello:^1.0.0"`. Admission pins each created Pod to a digest at the node-local proxy. Logs show the hello program; the final command shows the resolved reference. Changing a range or channel does not restart existing Pods.

## Image reference

Both resources use `apiVersion: kimage.dev/v1alpha1` and are namespaced. Authors declare an Image; the controller creates its ImageRelease. This complete build example uses a same-namespace ConfigMap:

```yaml
apiVersion: kimage.dev/v1alpha1
kind: Image
metadata:
  name: hello
  namespace: apps
spec:
  version: "1.0.0"
  source:
    build:
      format: Containerfile
      execution: {strategy: Direct, builderRef: default}
      cache: {mode: Local}
      definition:
        inline: |
          FROM scratch
          ARG TARGETARCH
          COPY hello-${TARGETARCH} /hello
          ENTRYPOINT ["/hello"]
      inputs:
        contextPath: /workspace/context
        volumes:
          - name: context
            configMap:
              name: hello-context
              defaultMode: 365
        volumeMounts:
          - {name: context, mountPath: /workspace/context, readOnly: true}
```

Decimal file mode `365` means octal `0555`. Select exactly one source: `source.build` or `source.ociArchive`. Version must be a complete semantic version with no build metadata. A changed output needs a new version.

| Field | Contract |
| --- | --- |
| `spec.version` | Exact immutable publication identity within the Image UID. |
| `spec.channels` | Explicit name-to-exact-version map, for example `{stable: "1.0.0"}`. It selects a published release; it does not build that mapped version. |
| `spec.platform` | Optional single native platform, Linux/amd64 or Linux/arm64. Omit for automatic selection. Use `spec.platforms` for an explicit set. |
| `definition` | One inline Containerfile or `fromFile: {path: ...}` from a declared mount. |
| `execution` | `Direct` with administrator-approved `builderRef: default`. No author-selected engine address or arbitrary engine options. |
| `cache.mode` | `Local` reuses disposable worker intermediates; `Disabled` requests a real no-cache solve. |
| `spec.exports` | Optional destinations, independent of internal readiness and build identity. |

`status.conditions`, `observedGeneration`, `operation`, `releaseRef`, and `build` describe reconciliation. `status.exports` tracks each destination separately. A release binds the Image UID, version, original OCI root descriptor, platform, and provenance; only verified published releases are selectable.

**Import an existing OCI archive**

Use a same-namespace PVC with a relative archive path, expected root descriptor (`mediaType`, `digest`, `size`), and `producerHandoff: true`. The archive must be readable by UID 65532. Obtain the descriptor from the artifact producer; an archive-file checksum is not the OCI root digest.

```
# Inside spec, instead of source.build:
source:
  ociArchive:
    claimName: image-input
    path: service.oci.tar
    producerHandoff: true
    root:
      mediaType: application/vnd.oci.image.manifest.v1+json
      digest: sha256:<actual-manifest-digest>
      size: <actual-manifest-byte-count>
```

Single-platform manifests and nested indexes are verified without rewriting their bytes. Truncation, wrong sizes/digests/platforms, missing graph objects, and invalid DiffIDs fail verification. Multi-platform images, zstd layers, descriptor URLs, embedded data, and referrer graphs are unsupported.

For the complete generated CRD schemas, run `kimage-install schemas`. This prints schema JSON and does not install it; the operator owns guarded schema establishment.

## Native platforms and build policy

Omit `spec.platform` for automatic native amd64/arm64 selection. Admission evaluates the actual Pod's node name, architecture selector, and required node affinity against eligible Ready nodes. All managed containers contribute compatible platform constraints.

| Request | Result |
| --- | --- |
| Placement permits one architecture | Build and use only that native variant. |
| Both architectures remain eligible | Build both; pin a fully verified OCI index. Kubelet chooses its native child. |
| No consumer, default OnDemand policy | Image stays AwaitingDemand; no automatic build. |
| No consumer, Eager policy | Prepare all available native platforms. |
| Explicit platform or platforms | Directly request the specified available native variants. |

The administrator ConfigMap `<installation>-config` in the system namespace contains `data.configuration.json`:

```
{"schema":1,"buildPolicy":"OnDemand"}
```

Change `OnDemand` to `Eager` to prebuild automatic Images. The operator watches this setting and installation preserves it. Explicit `spec.platform: {os: linux, architecture: arm64}` requests one platform. Use `spec.platforms` for a list of native platform objects, with `platform` omitted.

A real missing-release Pod CREATE records bounded UID/generation/version demand in an `ImageDemand`, then returns `ImageBuildPending`. Normal workload controllers retry while the operator builds. For a direct Pod apply, wait for the Image and retry. Server dry-run records no demand. Apply the automatic Image and its Deployment before waiting for readiness.

Versions are immutable per Image UID and native platform, with a separate aggregate identity. `status.platformReleases` lists native variants. A later consumer can request an additional variant; the index is published only after both complete graphs and matching private input captures are verified. Changed input captures require a new version.

The CLI and Helm discover one native worker per available architecture and a read proxy for every eligible node. Kubelet selects native children from the pinned component indexes. From 0.5.0, the operator automatically registers eligible nodes and platforms under the configured node policy. Earlier releases require reapplication after node additions. There is no QEMU emulation. Provide matching executable inputs for every requested application architecture.

## Namespace or cluster-wide installation

The default `--scope namespace --application-namespace apps` manages one application namespace. Use `--scope cluster` and omit `--application-namespace` for all ordinary namespaces, including namespaces created later. Both require cluster administrator setup for shared CRDs, webhooks, and RBAC.

Cluster mode excludes its system namespace, `kube-system`, `kube-public`, and `kube-node-lease`. Repeat `--exclude-namespace NAME` for additional exclusions. Overlapping installations are refused; retain the same scope and flags when resuming installation or recovery.

Images, input mounts, destination credentials, and releases stay namespaced. Even cluster-wide, a Pod in `apps-a` cannot use `platform://apps-b/service:1.0.0`. Each namespace can define its own `service` Image. Namespace scope uses application Roles; cluster scope grants broader application permissions with runtime namespace filters. Node administrators are trusted registry readers.

Native Kubernetes E2E runs CLI and Helm, both installation scopes on amd64 and arm64, covering real build/pull, policy, workload controllers, RBAC, namespace boundaries, exclusions, and retained storage on uninstall. The release workflow requires this acceptance matrix.

## Inputs, secrets, and build boundaries

Declare read-only ConfigMap, Secret, or PVC volumes and mounts. ConfigMap/Secret selections are frozen at their UID/resourceVersion into attempt-owned immutable objects. Context files and the Containerfile have separate roots: moving a mounted definition does not change what `COPY` can read.

- **Ordinary context:** map text/binary keys and modes beneath the declared `contextPath`. Overlap, traversal, writable inputs, unsupported volume sources, and symlink escapes are rejected.
- **Mounted definitions:** ConfigMap paths live below `/workspace/inputs`; Secret paths below `/run/kimage-build-secrets`. Select the file using `definition.fromFile.path`.
- **PVC capture:** set `inputs.producerHandoff: true`, stop producers, use a read-only claim/mount, and optionally select `subPath`. Known writers and binding/placement changes are checked. This is a handoff contract, not an atomic filesystem snapshot.
- **Secret bindings:** `buildSecrets: [{id: token, sourceFile: /run/kimage-build-secrets/auth/token}]` binds a declared Secret file to `RUN --mount=type=secret,id=token ...`.

Secret files stay outside the ordinary context; secret definition bytes and sensitive fingerprints stay out of normal logs, status, and release provenance. A recipe can still deliberately write a secret to its output. Avoid credentials in recipe literals, image history, or ordinary context files.

Secret values do not automatically invalidate BuildKit cache. Set a non-sensitive `cache.invalidationToken` when output depends on a changed secret; the initial adapter disables cache for such solves. Cache mode or placement alone does not force a published release to rebuild.

**Offline adapter:** provision all inputs in advance. The supported build uses `FROM scratch`, local input files, and network-none RUN. Remote bases, remote ADD/context URLs, external syntax frontends, remote caches, and general base-image provisioning are unavailable.

Cancellation terminates the owned client and fences the dedicated worker under its execution lease, discarding disposable cache. A finalizer waits for build cleanup on Image deletion. Operator restart resumes durable attempt state and fences an expired attempt before starting another.

## Version selection

Quote `platform://` selectors in Pod specs or workload Pod templates. Namespace must match the consuming Pod's namespace.

| Selector | Selects |
| --- | --- |
| `platform://apps/hello:1.2.4` | Exactly 1.2.4. |
| `platform://apps/hello:^1.2.0` | Highest compatible published release below 2.0.0. |
| `platform://apps/hello:~1.2.0` | Highest published release in the 1.2 patch series. |
| `platform://apps/hello:stable` | The exact version explicitly mapped in `spec.channels.stable`. |
| `platform://apps/hello?version=^1.2.0` | The equivalent readable query form. |

Colon ranges start with an operator (`^ ~ > < =`) or `*`. Compound ranges such as `"platform://apps/hello:>=1.2.0, <2.0.0"` work. Numeric-leading colon selectors must be complete exact versions; `:1.2` and `:1.2.*` are rejected. Prereleases need an explicitly matching prerelease requirement.

Query ranges accept literal characters or percent encoding decoded once, including the older `?version=%5E1.2.0` form. Colon ranges use literal characters. Do not combine colon and query selectors. There is no implicit latest or cross-namespace consumption.

Admission resolves actual Pod CREATE requests, including init containers, to digest references with `IfNotPresent` and eligible-node constraints. Templates remain symbolic. Channel/range changes affect new Pods; existing Pods keep their digest. Use exact versions for a uniform rollout. Unsupported image updates and ephemeral-container paths are rejected; create replacement Pods through the workload controller.

### Workload compatibility

Put `platform://apps/hello:^1.0.0` in the container image field of your workload. Resolution happens when its controller creates a Pod, so controllers need no kimage-specific plugin. Normal containers, init containers, and native sidecars (`initContainers` with `restartPolicy: Always`) follow the same path.

| Resource | Where to put the selector |
| --- | --- |
| Pod | `spec.containers[].image` or `spec.initContainers[].image` |
| Deployment, StatefulSet, DaemonSet, ReplicaSet, ReplicationController, Job | `spec.template.spec.containers[].image` or the equivalent `initContainers` field |
| CronJob | `spec.jobTemplate.spec.template.spec.containers[].image` or the equivalent `initContainers` field |
| Custom controller | Its documented image or Pod-template field, provided the controller copies the selector into a generated Pod |

```
apiVersion: batch/v1
kind: CronJob
metadata:
  name: report
  namespace: apps
spec:
  schedule: "0 * * * *"
  jobTemplate:
    spec:
      template:
        spec:
          restartPolicy: Never
          containers:
            - name: report
              image: "platform://apps/report:^1.0.0"
```

For this CronJob, publish a `report` Image whose default command finishes after one run. The tutorial's long-running hello executable is intended for a Deployment.

The Image and generated Pod must be in the configured application namespace, even if the controller runs elsewhere. A custom resource's schema, validation webhooks, and controller must accept the selector unchanged. Controllers that parse image references themselves, inspect an external registry first, or rewrite image strings may need an adapter. kimage does not scan arbitrary custom-resource fields or strings in ConfigMaps, environment variables, build definitions, or container arguments.

Update the workload template to select another image; its ordinary rollout or scheduling policy creates new Pods. Changing a channel or publishing another matching version does not itself trigger a rollout. Managed ephemeral-container updates, image-backed volume references, static Pods, and execution outside Kubernetes Pod admission are currently unsupported.

The application webhook is fail-closed. An outage can block other Pod creation in its namespace too. Direct internal registry references are denied in application scope, and an opt-in Pod label is not a bypass.

## Optional registry export

Export copies a verified retained release without rebuilding it. An administrator enables the separate exporter with repeated `kimage install --export-origin https://registry.example/` flags, or Helm `exporter.approvedOrigins`. No configured targets means no external registry requests.

```
# Add under spec on an Image:
exports:
  - name: production
    registry:
      repository: registry.example/team/hello
      tags: ["1.0.0"]
      credentialsSecretRef: {name: registry-push}
      caSecretRef: {name: registry-ca}
      overwritePolicy: Never
```

The same-namespace credential Secret uses type `kubernetes.io/dockerconfigjson` and an `auths` entry for the exact authority, including any port. Only HTTP Basic credentials or explicit `unauthenticated: true` are supported. Bearer/token exchange, credential helpers, and registry-specific login flows are unavailable. Omit the CA Secret for publicly trusted certificates; a private issuer Secret contains `ca.crt`.

HTTPS hostname validation, fixed-origin upload locations, and redirect refusal remain enabled. The exporter preserves original OCI bytes, verifies remote readback, and attaches explicit tags. `Never` accepts an existing identical tag and rejects a differing one as `TagConflict`. Installation-local Leases serialize its writers; strong global tag immutability still needs registry enforcement against other actors.

Inspect `status.exports` for phase, selected release UID, local/remote digest, reference, and retry reason. Retries are bounded and credential/configuration changes reset retry identity. Export failure does not change local readiness. Removing a target or Image stops future work without deleting remote content.

## Compatibility and alpha limits

| Layer | Tested profile |
| --- | --- |
| Native CI acceptance | Native Linux/amd64 and Linux/arm64 E2E; Kubernetes `v1.36.5+k3s1`; containerd `2.3.4-k3s1.36`; native Ubuntu 24.04 runners. Acceptance receipts record each runner's kernel. Select `--runtime-profile k3s`. |
| Mixed-platform acceptance | RKE2 `v1.36.5+rke2r1`; containerd `2.3.4-k3s1`. Native amd64/arm64 placement, builds and pulls, plus 0.5.1 CLI/Helm and Argo CD upgrade, downgrade, interruption, node-change and retention acceptance. Select the default `inferal` profile. See the [0.5.1 receipt](https://github.com/inferal-oss/kimage/blob/main/docs/mixed-0.5.1-results.json). |
| Builder | BuildKit 0.33.0, builtin Dockerfile frontend 1.27.0, OCI/runc worker, native snapshotter. |
| Serving | Distribution 3.1.2; gateway/proxy 0.3.0. Explicit rollback window described below. |
| Native clients | Rust 1.99.0; macOS and Linux/musl on amd64 and arm64. |

The installer checks exact runtime versions. A generic kind, k3d, or Docker cluster is not automatically supported: user namespaces, idmapped volumes, kernel/filesystem/runtime support, CNI policy, storage all matter.

The worker uses `hostUsers: false`, nested namespace capabilities, unconfined seccomp/AppArmor inside its Pod, no host paths/runtime sockets, and no service account token. It is administrator managed; hostile tenant isolation is not claimed. Unsupported policy fails without a privileged fallback.

One retained PVC/registry is authoritative, with RWO/topology/capacity limits. There is no HA, arbitrary storage/schema migration, signing/attestations, or automatic retained-artifact GC. A registry outage leaves running containers alone but can block new uncached starts.

## Cluster setup

Supply administrator kubeconfig, Ready Linux amd64/arm64 nodes, persistent storage and the documented runtime/user-namespace prerequisites. Nodes need HTTPS access to GHCR or your mirror; the CLI needs GitHub access for the small catalog. No carrier image or credential-generation script is needed.

### Install with the CLI

```
kimage install --application-namespace apps

# Tested K3s profile, explicit storage:
kimage install --application-namespace apps \
  --runtime-profile k3s --storage-class local-path

# All ordinary namespaces, with an extra exclusion:
kimage install --scope cluster --exclude-namespace infrastructure
```

The installer creates missing requested namespaces, generates separate service identities and applies ordinary resources. Reapply the same command after interruption. It reuses credentials and preserves the administrator's build policy. Existing foreign resources, scope overlap and a different installation manager are refused. Both installation scopes need administrator access for shared CRDs and webhooks.

### Install with Helm

```
helm repo add kimage https://kimage.dev/charts
helm repo update
kubectl create namespace apps
helm install kimage kimage/kimage --version 0.5.2 \
  --namespace kimage-system --create-namespace \
  --set applicationNamespace=apps --wait --wait-for-jobs
```

Use Helm 3.14 or newer. Helm queries live Ready Nodes during install/upgrade, and does not create the application namespace. Add `--set runtimeProfile=k3s` for the tested K3s profile. For cluster scope, omit the application namespace and set `--set scope=cluster`. Use a values file for exclusions.

| Helm value | Default / meaning |
| --- | --- |
| `applicationNamespace` | `default` in namespace scope |
| `scope`, `excludedNamespaces` | `namespace`; extra cluster-scope exclusions |
| `runtimeProfile` | `inferal`; use `k3s` for the tested K3s profile |
| `persistence.storageClass`, `persistence.size` | Cluster default, `1Gi` |
| `persistence.existingClaim` | Optional existing Bound claim |
| `registryNode` | First eligible node initially; retained on reapply |
| `buildPolicy` | Empty preserves the existing setting, otherwise `OnDemand`; can set `Eager` |
| `port` | `17701`; distinct for installations sharing nodes |
| `images.repository`, `images.digests` | Official GHCR repository and eight release-pinned indexes |
| `exporter.approvedOrigins` | Empty disables export; explicit HTTPS origins enable the separate exporter |
| `workerNodes` | Optional initial architecture-to-node pins |
| `nodePolicy` | Node label selection and worker pins; null preserves administrator edits |
| `nodes` | Full Node objects only for offline rendering; normally discovered live |

The operator owns shared CRD compatibility checks; the chart does not delete those CRDs. The source chart has empty image pins until release packaging verifies both native graphs. Published charts are ready to install.

## Install with Argo CD

From 0.5.1, set `gitops.enabled: true` on the published Helm chart. Rendering is deterministic and offline. An in-cluster provisioner discovers native nodes, generates credentials once and waits for authenticated readiness. Argo CD manages desired state; kimage manages its runtime resources.

```
apiVersion: argoproj.io/v1alpha1
kind: Application
metadata:
  name: kimage
  namespace: argocd
  finalizers:
    - resources-finalizer.argocd.argoproj.io
spec:
  project: default
  source:
    repoURL: https://kimage.dev/charts
    chart: kimage
    targetRevision: 0.5.2
    helm:
      releaseName: kimage
      valuesObject:
        gitops:
          enabled: true
        scope: cluster
  destination:
    server: https://kubernetes.default.svc
    namespace: kimage-system
  syncPolicy:
    automated:
      prune: true
      selfHeal: true
    syncOptions:
      - CreateNamespace=true
```

For one application namespace, use `scope: namespace` and `applicationNamespace: apps`. The provisioner creates that namespace when missing. Use the [cluster Application](https://kimage.dev/examples/argocd/cluster.yaml) or [namespace Application](https://kimage.dev/examples/argocd/namespace.yaml). The AppProject must permit the chart's administrator installation RBAC and destination. Both scopes use a privileged provisioner; runtime identities enforce the selected application scope.

### Configuration and stable credentials

Use normal chart values for runtime profile, storage, mirrors, exports and placement. Do not put full Nodes or inline credentials in Git. Optional `gitops.credentialSecrets` maps `registry`, `admission` and `builder` to existing Secret names in the system namespace. Missing roles are generated inside the cluster. Credentials are reused across syncs and restarts; changing supplied material is refused instead of silently rotating it.

Keep `buildPolicy` empty and `nodePolicy` null to preserve administrator edits, or set them explicitly to manage policy from Git. Use `nodePolicy.workerPins` for fixed native worker placement. Registry Node UID and volume placement never move automatically.

### Readiness and version changes

Wait for Healthy and Synced. Provisioner readiness includes component and doctor checks. Inspect safely bounded failure reasons and installation health:

```
kubectl -n argocd get application kimage
kubectl -n kimage-system logs deployment/kimage-provisioner
kimage status
kimage doctor
```

Interrupted provisioning retries automatically. Compatible version changes use the lifecycle Lease, exact-target journal and retained-identity checks. Keep the desired target unchanged while an operation is pending. Pin an exact chart version; a future release must declare and test the transition. Argo CD ownership starts at 0.5.1, so the 0.5.0 chart and runtime are not Argo CD rollback targets. Existing CLI/Helm installations are not adopted automatically. The current adjacent release edge is 0.5.1 to 0.5.2 and back; earlier tested edges remain supported.

From 0.5.1, change the exact `targetRevision` for compatible upgrades or downgrades. The returning older provisioner automatically runs a short-lived Job using the installed newer operator’s recorded digest. That binary checks its exact compatibility edge and uses the same lifecycle Lease and journal. Both helper and provisioner interruptions resume automatically; keep the requested version unchanged until completion. No manual catalog override is needed. Unsupported edges remain refused.

Provisioners and transition Jobs need Kubernetes API, DNS and HTTPS access to the included registry Service on port 5443. Retained-byte checks use its CA and reader credentials; no API port-forward permission is required. For `GitOpsTransitionPending`, inspect Jobs with label `kimage.dev/gitops-transition=<installation>` and their bounded reason codes.

### Deletion and retained data

Application deletion or pruning removes the provisioner and desired-state resources, leaving the runtime, registry PVC, Images, ImageReleases and shared CRDs. Runtime objects do not inherit Argo CD tracking or ownership from the provisioner. Recreating the same Application resumes with the same credentials and data.

For runtime removal, first delete the Application and wait for its provisioner Deployment, provisioner Pods and transition Jobs/Pods to disappear, then run `kimage uninstall --namespace kimage-system --installation kimage --allow-argocd`. Argo-managed uninstall requires terminal confirmation or `--allow-argocd`. This flag acknowledges ownership and does not bypass lifecycle safeguards. Uninstall refuses while the provisioner Deployment or transition resources are present and preserves retained data. Deleting the system namespace discards that protection.

The [complete Argo CD guide](https://github.com/inferal-oss/kimage/blob/main/docs/argocd.md) lists credential keys, advanced target catalogs, retention and ownership boundaries. No repo-server plugin, broad diff exclusions or manual credential-generation script is required.

## Reapply, upgrade and rollback

### Find installations and check health

```
kimage installations
kimage status
kimage doctor
kimage installations --namespace kimage-gitops-system --json
```

Commands use the current Kubernetes context. If `kimage` in `kimage-system` is missing, status and doctor discover and inspect every matching installation, continuing after an individual failure. Upgrade, rollback and uninstall also discover, but require an explicit choice when several installations match. A damaged existing record or a permission/API error does not trigger fallback.

Use `--namespace SYSTEM` or `--installation NAME` to constrain discovery; both flags select an exact identity. Add `--no-discovery` to require the selected/default target without searching. Cluster discovery requires ConfigMap list access across namespaces; namespace-only search needs list access there. Exact selection avoids cluster listing. Install keeps its creation defaults; logs/builds select an Image in an application namespace.

Single status/doctor JSON keeps the existing object. Several results use `{"schema":1,"healthy":false,"installations":[...]}`, with aggregate health and an identity and error for each failed inspection. Doctor exits nonzero if any installation needs attention; status exits nonzero if an inspection fails. `installations --json` always returns an installations array, which can be empty.

### Guarded version transitions

Install the new CLI first; this changes no cluster components. The supported release edge is **0.5.1 to 0.5.2 and back**. Use the same guarded commands for CLI and Helm installations:

```
kimage upgrade --to 0.5.2 --check
kimage upgrade --to 0.5.2
kimage status --json
kimage doctor --json
kimage rollback --to 0.5.1 --check
kimage rollback --to 0.5.1
```

Add `--namespace` and `--installation` for a non-default system identity. Checks are read-only. The operation records exact catalogs and resumable phases, stops new reconciliation, terminates old clients and workers, and replaces components under an exclusive Lease. Completion verifies readiness and retained storage, CRD and credential identities. New builds, managed Pod admission and uncached pulls may pause; running application containers remain under their workload controller.

If interrupted, repeat the exact command and catalog. A crashed process's Lease expires after 30 seconds. Do not delete the journal or switch targets. `status` shows the pending phase; `doctor` checks drift, native readiness and certificate validity and exits nonzero when attention is needed. A failed operation does not silently roll back.

For Helm ownership, keep Helm 3 or 4 on PATH. The commands invoke Helm with exact target image pins and preserve revision history. Helm 4 transitions select ordinary three-way updates to coordinate with operator quiescing and reconciled node resources. Rollback records a new Helm revision from the exact older chart and current node placement. New charts reject unguarded cross-release upgrades. Old charts cannot add guards to raw Helm rollback, so use the guarded path for compatibility checks and interrupt recovery. Inspect history with `helm history kimage --namespace kimage-system`.

The release asset `release-compatibility.json` records exact tested transitions and persisted formats. Unknown edges are refused. Before returning to 0.4.1, restore an empty node selector; that operator cannot enforce the newer selection policy. Worker pins and build policy are preserved.

Helm preflight checks that stable history can recover an interrupted revision without restoring stale credentials or build policy. If it reports `HelmRecoveryCredentialDrift` or `HelmRecoveryPolicyDrift`, reapply the current version's chart to record the live values before checking the upgrade again.

### Automatic node membership

The operator registers eligible new nodes and native architectures automatically. Existing automatic workers stay in place while eligible. Removed membership revokes UID-bound serving approval, and worker replacement fences old clients before creating a replacement. A partitioned worker can leave fencing pending. Registry node identity and retained storage never move automatically.

If doctor reports `NodeReconcileForbidden`, compare the operator's system namespace Role and RoleBinding with the exact release's manifests. Node ownership updates need Pod and Job metadata patch permission in that namespace.

The administrator ConfigMap `<installation>-node-policy` uses label `kimage.dev/installation: <installation>` and the following `policy.json`:

```
{"schema":1,"nodeSelector":{"kimage.dev/pool":"images"},"workerPins":{"arm64":"arm-worker"}}
```

Empty selection includes all eligible nodes. Pins require the named eligible node instead of silently moving work. The registry node must remain selected. Helm's `nodePolicy` value sets this policy; null preserves administrator edits. On first install, explicit `workerNodes` values become pins.

Release transitions keep scope, storage, ports and installation identity. They do not skip releases, downgrade shared CRDs, migrate storage or adopt 0.3.x bootstrap installations. `kimage install` remains a same-release resume command. Retain original installers and trusted bundles for [legacy recovery](https://github.com/inferal-oss/kimage/blob/main/docs/operations.md).

## Recovery and retained storage

System components can restart by pulling their public or mirrored images independently of the included application registry. Restore service by reapplying the same installation and inspecting normal Pod events. This does not recover lost storage. Back up the complete Distribution backend together with release metadata, UIDs and schema coordination records.

```
kimage uninstall --namespace kimage-system --installation kimage
# For a Helm-managed installation:
helm uninstall kimage --namespace kimage-system --wait
```

After rollback to a predecessor such as 0.5.0 or 0.4.1, stop its operator and remove its named workers before Helm removal to prevent reconciliation from racing chart deletion. Version 0.4.1 can recreate workers without Helm tracking metadata. The CLI uninstall command already stops supervisors first. Use your installation name and namespace:

```
kubectl -n kimage-system scale deployment/kimage-operator --replicas=0
kubectl -n kimage-system wait --for=delete pod -l kimage.dev/installation=kimage,kimage.dev/role=operator --timeout=180s
kubectl -n kimage-system delete pod kimage-builder-amd64 kimage-builder-arm64 --ignore-not-found --wait=true
helm uninstall kimage --namespace kimage-system --wait
```

Removal keeps PVCs, Images, ImageReleases and shared CRDs. Do not remove namespaces, release pins, finalizers or registry backends during ordinary cleanup. Running containers survive registry downtime; new uncached application starts wait or fail until serving returns. This remains a single-registry deployment, not HA.

Version 0.4.1 fixes intermittent `CleanupIdentityConflict` errors during CLI uninstall when Pod status changes between LIST and DELETE. UID, specification and ownership checks remain enforced. For an existing 0.4.0 installation, update the CLI with the Cargo command above, then run the same uninstall command. This cleanup fix requires no component reinstallation or data migration.

## Credentials

CLI installation generates registry, admission and worker credentials in process. Helm uses its standard certificate functions. Each service has a separate CA; registry read and publish passwords are independent, and admission has its own resolution key. CA private keys are not deployed. Worker endpoints use authenticated TLS.

The complete base Secrets are created before their derived views, so interrupted setup resumes with matching server/client identities. Reapplication reuses owned Secrets and preserves their identities. Helm stores release resources in its normal release history; protect that history as credential material.

Generated leaf certificates last ten years. Reapplication preserves them and is not automatic renewal. Plan certificate maintenance before expiry. CA/password/key rotation needs an explicit maintenance and storage-recovery plan; legacy `renew-tls` applies to older bootstrap installations, not the new default.

## Observe and troubleshoot

### Build logs and failed attempts

```
kimage builds hello --namespace apps
kimage logs hello --namespace apps --platform linux/arm64 --follow
kimage logs hello --namespace apps --attempt <full-attempt-id>
```

`builds` lists active and retained attempts as JSON. Use the original Image name for automatic multi-platform builds. Select a platform or attempt when needed. Follow stays with that attempt through completion or failure.

Each attempt keeps a 64 KiB text tail with an explicit truncation marker. Cleanup archives output in a namespace Secret before removing the client Pod. The latest five attempts per Image and platform survive retries and operator restarts. Deleting the Image removes its logs. Collection can be unavailable after worker or API loss; 0.4.1 attempts have no captured output.

During worker recovery, demand waits for worker Pod and service endpoint readiness before capturing inputs. Normal replacement startup does not create an empty interrupted attempt.

The adapter also limits raw progress to 16 MiB per solve and 1 MiB per progress record. Exceeding either fails the attempt with `BuildProgressLimit` or `BuildProgressLineLimit`; its retained tail stays bounded to 64 KiB. BuildKit may clip step output upstream.

Recipes can print secrets. Retained logs require Secret get/list permission; live output also requires Pod get/list and exec. Raw output stays separate from status, release metadata, operator logs and metrics. Arbitrary user output is not automatically redacted.

Image conditions report stable failure reasons. Transient failures retry with persisted backoff. Permanent definition or solve failures wait for changed declarations or inputs. Restarting the operator does not reset them. `VersionConflict` needs a new version, and a successful solve still needs publication and readback.

Start with the resource condition, the installer phase, and normal Kubernetes events. Avoid changing storage, deleting release metadata, or bypassing TLS to work around a failed prerequisite.

```
kubectl -n apps get images,imagereleases
kubectl -n apps get image hello -o yaml
kubectl -n apps describe pod <pod-name>
kubectl -n kimage-system get pods
kimage install --help
```

| Symptom | Next check |
| --- | --- |
| Image not Ready | Read conditions and observed generation. Confirm builder readiness, source readability, producer handoff, and durable retry eligibility. |
| `VersionConflict` | The UID/version already has different content. Publish a new exact version. |
| Pod rejected at admission | Check namespace, explicit selector/channel, a matching verified release, and approved ready node proxies. The webhook is fail-closed. |
| ImagePullBackOff | Check proxy and registry readiness, TLS/credentials, node eligibility, network policy, and retained storage. Running containers can survive a serving outage. |
| Worker fails to start or RUN fails | Confirm exact runtime profile, user namespace/idmapped volume support, kernel/filesystem policy, and nested execution permissions. |
| Export failed | Read `status.exports`: approved HTTPS origin, exact authority credentials, CA, tag conflict, and retry eligibility. Local publication is independent. |

**Metrics and freshness**

Operator/admission expose `/metrics` on existing health listeners. Proxy 0.3.0 exposes it on its loopback listener with canonical Host authority. Inspect through API port forwarding; metrics do not add an external diagnostics interface.

`kimage_stage_duration_seconds_count/sum`, `kimage_stage_active`, `kimage_stage_failures_total`, and `kimage_stage_transferred_bytes_total` cover build, publication/readback, and proxy pulls using fixed stage labels. Durations are process-local summaries without quantiles; bytes measure observed transfers, not unique storage or guaranteed client consumption.

Check `kimage_reconcile_waiting`, `kimage_images_pending`, `kimage_images_ready`, and `kimage_releases_verified`. Interpret catalog counts with `kimage_catalog_sample_complete`/`kimage_catalog_sample_timestamp_seconds`; incomplete samples retain earlier counts. Admission node counts use `kimage_nodes_eligible` and `kimage_node_sample_timestamp_seconds`. Labels exclude image names, UIDs, digests, input contents, and credentials.

**Native client API access overrides**

For an independently revalidated API access path, set both `KIMAGE_API_SERVER=https://<address>:<port>` and `KIMAGE_TLS_SERVER_NAME=<certificate-name>`. Native clients retain kubeconfig CA/credentials and refuse insecure verification. These variables do not edit kubeconfig or node configuration.

## Architecture and trust

The operator publishes complete verified graphs before sealing immutable release metadata. Publication is not one atomic registry/API transaction: durable operation identity, topological uploads, independent readback, and guarded status writes make retries recoverable.

Admission reads a coherent catalog and approved node records. Real Pod creation can record bounded missing-platform demand; admission does not build or publish. Kubernetes applies its returned Pod patches for digest references and eligible-node constraints. Kubelet uses the normal runtime to pull from `127.0.0.1:<port>/<namespace>/<image-uid>@sha256:...`.

The proxy allows digest-only GET/HEAD with a fixed verified HTTPS upstream and separate read identity. Distribution binds Pod loopback behind its TLS gateway. Publisher, reader, operator, admission, input, and exporter permissions are separate. Registry/proxy/input processes need no Kubernetes token.

Eligible node administrators are trusted readers; namespace admission is not image confidentiality against hostile tenants on the same node. CNI policy must enforce worker egress denial; proving runtime-level disconnection requires a network boundary outside the Pod. Retained storage and the Kubernetes control plane remain infrastructure dependencies.

## Where other approaches fit

These tools have different scopes. A configured registry can be local or remote, and integrations can add triggers or deployment automation.

| Approach | Core workflow |
| --- | --- |
| kimage | Declare an exact versioned Image; controller manages production, retained local publication, and integrated digest resolution for workloads. |
| [Shipwright](https://shipwright.io/docs/build/) | Build definitions and BuildRuns with configurable strategies and an explicitly configured output registry. |
| [Tekton](https://tekton.dev/docs/pipelines/taskruns/) | General task/pipeline execution through runs; build/push steps and deployment integration are configured in that workflow. |
| [kpack](https://github.com/buildpacks-community/kpack) | Declarative Images using Cloud Native Buildpacks, with registry tags/credentials and rebuilds on source/builder changes. |

### kimage and kpack: image requests versus registry references

Both declare an `Image` and let a controller manage its production. Declarative images, digest-pinned output, and [ARM64 support](https://github.com/buildpacks-community/kpack/blob/main/docs/stack.md#building-for-arm64) are shared capabilities. kimage is an integrated **image catalog, builder, and delivery system**. Its distinction is connecting a workload's image and version request to retained releases and platform-appropriate delivery.

| Area | kpack | kimage |
| --- | --- | --- |
| Building | Cloud Native Buildpacks supply application language and runtime build logic. | Containerfiles with provisioned inputs, currently limited to `FROM scratch`. |
| Registry | Publish to a registry you configure and operate. It can be local. | A retained cluster-local registry and node-local serving are included. External export is optional. |
| Releases | Maintain an image as source, buildpacks, and stack change; expose the latest digest and build history. | Immutable Image identity/version/platform releases, selected by exact version, range, or channel. |
| Workloads | Deployment tooling consumes the resulting registry reference. | Workloads declare an Image name and accepted versions; admission resolves each new Pod to a digest. |
| Demand | Rebuild on source, buildpack, and stack changes. | Default to consumer demand. Placement selects platforms; administrators can enable eager builds. |

A kpack build exposes a registry reference in `status.latestImage` for deployment tooling to consume. With kimage, the workload can instead keep this request:

```
image: "platform://apps/service:^1.2.0"
```

This asks for the highest compatible published `service` version below `2.0.0` in `apps`. The workload author does not supply a registry hostname, repository path, or digest. Admission selects a verified retained release and pins the new Pod to its digest, using placement to select native output or a multi-platform index. The runtime still performs normal registry pulls through node-local serving.

**The registry is included.** Application authors do not choose a separate publication service or configure output push credentials per Image. Administrators provision the included registry's persistent storage; installation generates service credentials. Exporting to another registry is an optional additional operation.

Version selection happens on new Pod creation. Existing Pods keep their digest; publishing another matching version does not trigger a rollout. Selectors remain namespaced, and missing platform outputs follow the [build policy](https://kimage.dev/docs/#platforms). See [version selection](https://kimage.dev/docs/#selectors) for ranges, channels, and pending-demand behavior.

kpack has the more established source-to-image workflow through Buildpacks. See its [Image model](https://github.com/buildpacks-community/kpack/blob/main/docs/image.md) and [registry tutorial](https://github.com/buildpacks-community/kpack/blob/main/docs/tutorial.md). kimage's current builder has narrower input and base-image support; its focus is the integrated path from image declaration to version-selected delivery in a workload.

## Development and validation evidence

The source is Apache 2.0 licensed. To work on kimage:

```
git clone https://github.com/inferal-oss/kimage.git
cd kimage
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
python3 scripts/check-docs.py
python3 scripts/check-site.py
```

CI runs Rust tests on macOS/arm64 and native Linux/amd64 and Linux/arm64 musl, plus legacy release-tooling and component/chart tests, formatting, Clippy, documentation/example checks, and package verification. These include parser/admission compatibility for readable colon and literal/encoded query ranges, native graph verification, placement and build policy. Native Kubernetes E2E tests CLI and Helm, both installation scopes, on each architecture before release.

An [explicitly enabled mixed-cluster E2E](https://github.com/inferal-oss/kimage/blob/main/docs/mixed-cluster-e2e.md) uses fresh Images for unknown, amd64, and arm64 workload placement. It checks exact native variants and native consumption of an OCI index or manifest. Standard CI validates the harness and skips the live test; running it requires an approved real mixed cluster. [Two independent runs passed](https://github.com/inferal-oss/kimage/blob/main/docs/mixed-cluster-results.json) with the official 0.3.0 binaries on real RKE2 amd64/arm64 nodes. Each case used a fresh Image. Unknown placement built both variants and ran both architectures from one index; explicit placement built only the requested platform. Both runs removed their consumer Deployments and Pods.

The [0.5.1 native acceptance matrix](https://github.com/inferal-oss/kimage/blob/main/docs/native-0.5.1-results.json) covers published 0.5.0 → 0.5.1 → 0.5.0 with CLI and Helm in all eight cases, plus four actual Argo CD cases. Argo changes chart versions from 0.5.1 to a separate future-version conformance fixture and back, including interrupted provisioner and downgrade-helper recovery. The fixture is not a public release.

The [0.5.1 real mixed-cluster acceptance](https://github.com/inferal-oss/kimage/blob/main/docs/mixed-0.5.1-results.json) covers CLI/Helm 0.5.0 → 0.5.1 → 0.5.0 and both Argo CD scopes. Checks include interrupted chart upgrades/downgrades, native GHCR pulls, fresh unknown/explicit placement builds, node changes and complete OCI graph readback after removal/reinstall. Earlier unsuccessful attempts and the subsequent complete repeat remain recorded.

The [published 0.5.1 acceptance receipt](https://github.com/inferal-oss/kimage/blob/main/docs/published-0.5.1-results.json) verifies Cargo installation, automatic catalog fetching, public assets and the live Helm repository. A real Argo CD v3.5.3 test uses the published chart's own image pins, without image overrides, and confirms native builds and retained data after removal/reinstall.

The [0.5.0 native lifecycle matrix](https://github.com/inferal-oss/kimage/blob/main/docs/native-upgrade-results.json) covers published 0.4.1 → 0.5.0 → 0.4.1 with CLI and Helm, both scopes and architectures. Interrupted upgrades and rollbacks resume, private failed-build output survives client cleanup, and retained image graphs remain readable after rollback and uninstall/reinstall. The [real mixed-cluster round trips](https://github.com/inferal-oss/kimage/blob/main/docs/mixed-upgrade-results.json) also cover normal public GHCR pulls, fresh unknown/explicit placement builds and reversible node eligibility and worker pin changes. Shared Nodes are never deleted or cordoned.

The published 0.4.0 CLI and public Helm chart also passed [real mixed-cluster acceptance](https://github.com/inferal-oss/kimage/blob/main/docs/mixed-install-results.json): fresh Images for all three placement cases, native GHCR component pulls, and full retained OCI graph readback after uninstall and reinstall. The receipt records the published CLI's uninstall retry and the cleanup fix shipped in 0.4.1.

The separate owned two-node fixture passed installation, build/input matrices, normal uncached pulls, interruption and cold recovery, actual binary upgrade/rollback, and retained-release export. Those cluster harnesses are not ordinary CI jobs. Historical receipts predate later selector/CLI changes; they remain unchanged evidence for their recorded bundle.

For exact receipts and engineering contracts, see the repository's [requirements audit](https://github.com/inferal-oss/kimage/blob/main/docs/requirements-audit.md), [acceptance ledger](https://github.com/inferal-oss/kimage/blob/main/docs/status.md), and [release procedure](https://github.com/inferal-oss/kimage/blob/main/docs/releasing.md). Report problems through [GitHub issues](https://github.com/inferal-oss/kimage/issues). Include the runtime profile, relevant sanitized condition, and reproduction steps.

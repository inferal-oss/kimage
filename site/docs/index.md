# From an image declaration to a running workload.

A practical guide to producing, retaining, and delivering images inside Kubernetes. Start with the CLI and a small example; use the reference sections when you need them.

**Alpha, with a specific supported profile.** The CLI automatically fetches verified official release bundles. Cluster setup still needs an administrator, issuer credentials, persistent storage, a cached carrier, and the runtime prerequisites below.

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

With Rust 1.99.0 or newer and Cargo's bin directory on your PATH:

```
cargo install kimage-control --version 0.2.0 --locked
kimage install --help
```

No source checkout is needed. The package is `kimage-control`; the unrelated `kimage` crate belongs to another project. Add `--bin kimage` if you only want the installer.

| Command | Purpose |
| --- | --- |
| `kimage`, `kimage-install` | Verify, install, maintain, upgrade, and recover through Kubernetes APIs. |
| `kimage-operator` | Reconcile Images, build attempts, publication, and release metadata. |
| `kimage-admission` | Resolve workload selectors and enforce retained-release policy. |
| `kimage-gateway` / `kimage-proxy` | Authenticate registry traffic and serve node-local digest reads. |
| `kimage-input` / `kimage-exporter` | Capture inputs and optionally copy retained releases to another registry. |

Cargo compiles native commands for your machine. Official GitHub releases also provide prebuilt macOS/arm64 and Linux/amd64 installers and a complete Linux/amd64 component bundle. The installer fetches and verifies its matching release automatically; it does not provision a cluster. Gateway/proxy carry serving compatibility version `0.2.0`, independently of the Cargo package version.

## Automatic bundle delivery

Every bundle-using phase fetches the official release matching this CLI's exact package version, then reuses a verified local cache. `kimage install` handles this automatically, with no separate fetch/check step. It never selects `latest`. Optional: to prepare the bundle before you have a kubeconfig:

```
kimage bundle-fetch
kimage bundle-check --offline
```

The fetch command prints JSON containing `bundle` (the verified directory) and `trustedInventory` (its `sha256:` digest). It does not access Kubernetes. GitHub Actions builds and attaches the full bundle, separate inventory, both native installers, checksums, and license notices to each release.

| Option | Behavior |
| --- | --- |
| `--release 0.2.0` | Select an exact official release. Default: the installed CLI package version. Keep the same selection when resuming a phase. |
| `--bundle-cache <directory>` | Override the cache. Default: XDG_CACHE_HOME/kimage/bundles when set; otherwise ~/Library/Caches/kimage/bundles on macOS or ~/.cache/kimage/bundles on Linux. |
| `--offline` | Prevent bundle downloads. A cache miss fails with guidance; cached content is still reverified. Cluster commands still require access to Kubernetes. |
| `--bundle <directory>` | Use local bytes with a required independently trusted `--trusted-inventory sha256:<64-hex-digits>`. Cannot be combined with `--release`. |
| `--trusted-inventory sha256:...` | Optionally pin the inventory even for an official download/cache entry. |

Online trust comes from the fixed official `inferal-oss/kimage` repository over verified HTTPS: GitHub's API supplies each asset's SHA-256 and size. The client restricts download origins, verifies the compressed archive, rejects path traversal/links/duplicates and oversized entries, and checks the inventory, static executables, and complete OCI graphs before cluster mutation. Only a fully verified directory enters the cache atomically. Public downloads need no token. If API rate limits require authentication, `KIMAGE_GITHUB_TOKEN` applies only to the fixed metadata endpoint; it is never sent to asset downloads or redirects.

For an air gap, transfer the fetched `bundle` directory and its inventory digest through your approved channel, then supply both local arguments to the installation phases. Keep an independent copy for recovery when GitHub is unavailable. Cache corruption fails closed; preserve your trusted recovery copy before explicitly removing an invalid cache entry and refetching.

Official bundle assets start at `0.2.0`. Fetching `0.1.0` cannot supply assets that were never released; use its independently retained local bundle and installer instead.

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

On a Linux/amd64 machine with Rust and a musl linker installed (`musl-tools` on Debian/Ubuntu):

```
rustup target add x86_64-unknown-linux-musl
mkdir -p .local/tutorial
rustc --edition=2024 --target x86_64-unknown-linux-musl \
  -C opt-level=z -C strip=symbols -C panic=abort \
  examples/hello/main.rs -o .local/tutorial/hello
```

On macOS, use a Linux/amd64 build machine or container for this step. A native macOS executable cannot run inside the scratch image.

### 3. Provide input and request the image

```
kubectl -n apps create configmap hello-context \
  --from-file=hello=.local/tutorial/hello \
  --dry-run=client -o yaml | kubectl apply -f -
kubectl apply -f examples/hello/image.yaml
kubectl -n apps wait --for=condition=Ready image/hello --timeout=10m
kubectl -n apps get imagereleases
```

The example copies the executable into `FROM scratch` and requests version `1.0.0`. The controller handles execution and local publication. ConfigMaps suit small inputs; use a PVC for larger content.

### 4. Run it through a selector

```
kubectl apply -f examples/hello/deployment.yaml
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

Decimal file mode `365` means octal `0555`. Select exactly one source: `source.build` or `source.ociArchive`. Version must be a complete semantic version with no build metadata. A changed output needs a new version.

| Field | Contract |
| --- | --- |
| `spec.version` | Exact immutable publication identity within the Image UID. |
| `spec.channels` | Explicit name-to-exact-version map, for example `{stable: "1.0.0"}`. It selects a published release; it does not build that mapped version. |
| `spec.platform` | Currently Linux/amd64 only. |
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

The application webhook is fail-closed. An outage can block other Pod creation in its namespace too. Direct internal registry references are denied in application scope, and an opt-in Pod label is not a bypass.

## Optional registry export

Export copies a verified retained release without rebuilding it. An administrator explicitly starts the separate exporter using `kimage-install exporter` with the usual installation arguments and one or more `--approved-origin https://registry.example/` values. No configured targets means no external registry requests.

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
| Full acceptance cluster | Linux/amd64; Kubernetes `v1.36.5+k3s1`; containerd `2.3.4-k3s1.36`; Ubuntu 24.04.5, kernel `6.8.0-142`. Select `--runtime-profile k3s`. |
| Earlier foundation profile | RKE2 `v1.36.5+rke2r1`; containerd `2.3.4-k3s1`. The CLI's default profile is `inferal`; the complete later lifecycle proof uses the k3s fixture. |
| Builder | BuildKit 0.33.0, builtin Dockerfile frontend 1.27.0, OCI/runc worker, native snapshotter. |
| Serving | Distribution 3.1.2; gateway/proxy 0.2.0. Explicit rollback window described below. |
| Native clients | Rust 1.99.0; macOS/arm64 and Linux/amd64 musl. |

The installer checks exact runtime versions. A generic kind, k3d, or Docker cluster is not automatically supported: user namespaces, idmapped volumes, kernel/filesystem/runtime support, CNI policy, storage, and the cached carrier all matter.

The worker uses `hostUsers: false`, nested namespace capabilities, unconfined seccomp/AppArmor inside its Pod, no host paths/runtime sockets, and no service account token. It is administrator managed; hostile tenant isolation is not claimed. Unsupported policy fails without a privileged fallback.

One retained PVC/registry is authoritative, with RWO/topology/capacity limits. There is no HA, arbitrary storage/schema migration, multi-platform image support, signing/attestations, or automatic retained-artifact GC. A registry outage leaves running containers alone but can block new uncached starts.

## Cluster setup

This is an explicit administrator workflow. Keep a current and previous trusted bundle independently of the cluster, plus issuer material and a working administrator kubeconfig. Start on a disposable supported development cluster.

- **Prepare credentials:** separate registry, admission, and builder identities. See [credential files and service names](https://kimage.dev/docs/#credentials).
- **Confirm prerequisites:** exact runtime profile, selected node, approved digest-pinned carrier already cached there, storage class or existing claim, distinct system/application namespaces, and an available loopback port at least 1024.
- **Run one command:** `kimage install` fetches and verifies the bundle, then runs all standard phases. Preserve identical installation arguments for resume. Keep an independent recovery copy of the bundle and inventory digest.

**Optional: build a custom bundle from source**

Normal administrators use automatic release delivery. Maintainers and custom-bundle authors can use the repository packaging script with a prepared Linux/musl toolchain and independently authenticated upstream executables:

```
cargo build --locked --offline --release \
  --target x86_64-unknown-linux-musl --workspace --bins
python3 scripts/package-bundle.py \
  --linux-dir <linux-release-directory> \
  --registry <trusted-distribution-executable> \
  --registry-version 3.1.2 --serving-version 0.2.0 \
  --buildkit-dir <pinned-buildkit-directory> \
  --include-installers --macos-installer <macos-arm64-installer> \
  --out dist/complete-linux-amd64
kimage-install bundle-check --bundle dist/complete-linux-amd64 \
  --trusted-inventory sha256:<independently-trusted-64-hex-digits>
```

Pass trusted inventory digests as `sha256:` followed by 64 lowercase hexadecimal characters. An adjacent checksum alone does not authenticate origin. Verification checks inventory bytes, ELF architecture/static linkage, and complete component OCI graphs before cluster mutation. Native executable delivery uses bounded ConfigMap chunks; credentials never enter the OCI archives.

### Install into the cluster

Replace each placeholder with your approved value. No separate bundle-fetch, bundle-check, or phase commands are required.

```
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

The command validates all credential directories before bootstrap writes, then runs bootstrap, seed, handover, control plane, and the default builder. Rerun with identical arguments after interruption. It verifies the retained bundle and configuration checkpoints, resumes an interrupted handover, and checks readiness before continuing. `kimage-install` is a compatible alias.

**Advanced: run individual phases**

The following Bash function keeps the common arguments consistent. Replace every placeholder with your approved values; retain this configuration privately for resume/recovery. The example explicitly chooses the full-acceptance k3s profile.

```
ki() {
  kimage-install "$@" \
    --runtime-profile k3s \
    --namespace kimage-system --application-namespace apps \
    --installation dev --node <approved-node> \
    --carrier <cached-image@sha256:digest> \
    --storage-class <class> --port <loopback-port> \
    --credentials <registry-credential-directory> \
    --release 0.2.0
}
ki bootstrap
ki seed
ki handover
ki control-plane --admission-credentials <admission-directory>
ki builder --builder-credentials <builder-directory>
```

`bootstrap` starts carrier serving using API-delivered executables. `seed` uploads and independently reads back component graphs. `handover` warms ordinary digest pulls, replaces serving, then cleans executable payloads. `control-plane` validates schemas before registering admission; `builder` provisions and authenticates the default worker.

Use `--pvc <existing-claim>` when appropriate, alongside the required storage-class argument. Missing cached carrier or denied namespace policy is a prerequisite failure; administrator RBAC does not override admission policy. Foreign resources or changed installation configuration are refused.

The installer uses Kubernetes APIs and authenticated port forwarding. It does not SSH, edit node trust stores, mount the host filesystem, or access a runtime socket. Ordinary workloads pull through kubelet after bootstrap.

## Add nodes, upgrade, and roll back

Use `join-node` with the original installation/bundle arguments and `--target-node <additional-node>`. The target needs the same prerequisites and cached carrier. Endpoint approval follows ordinary proxy readiness and binds the Node UID, proxy digest, credentials, and scope. `--recover` permits replacement of an owned stale proxy after node replacement.

`upgrade-serving` takes target `--bundle`/`--trusted-inventory` and independently trusted source `--previous-bundle`/`--previous-trusted-inventory`, with the original installation arguments.

```
# Supply the usual installation arguments as in cluster setup:
kimage-install upgrade-serving <installation-arguments> \
  --bundle <target-bundle> --trusted-inventory <target-sha256> \
  --previous-bundle <source-bundle> \
  --previous-trusted-inventory <source-sha256>
```

**Rollback reverses those independently trusted source and target bundles.** Supported actual binaries are Distribution `3.1.1 ↔ 3.1.2` and gateway/proxy `0.1.0 ↔ 0.2.0`, with the shipped filesystem configuration and the same PVC. Prefer Distribution 3.1.2 for ordinary operation. Every non-serving component version/root and the bundle API revision must match.

A renewable Lease and immutable checkpoints serialize the transition. Replacement images warm through normal pulls before rollout; a read-only server uses the retained PVC while the single writer changes. Proxies overlap only after an authenticated configuration proof. Publication pauses during writer replacement; interrupted streams use normal client retry and digest checks.

Repeat the same transition after an interrupted installer's Lease expires. A checkpoint can restore a missing writer while read-only serving remains. If both serving paths are lost, use retained-bundle recovery first. The command never downgrades CRDs or migrates persisted data; other binary/storage transitions are refused.

The serving-only updater requires identical non-serving component versions and roots. Whole 0.1.0 and 0.2.0 release bundles differ there; automatic fetching does not turn them into a general operator/schema upgrade or downgrade path. Use matching independently retained bundles for recovery.

## Recovery, backups, and retention

`recover` with the original bootstrap arguments recreates carrier serving on the same PVC when ordinary serving is absent. Continue with seed/handover and the configured control-plane, builder, and joined-node phases. Independent bundles, cached carrier, usable nodes/control plane, and the original PVC remain prerequisites.

- **Back up together:** the entire Distribution backend, ImageRelease metadata and UIDs/root descriptors, and schema coordination state. Cache directories and source archives do not replace this backup.
- **Verify retained bytes:** `verify-remote` downloads and validates a complete graph without its source archive. Use `kimage-install verify-remote --help` for descriptor, repository, TLS, and authenticated API-forwarding arguments.
- **Retain releases:** published releases have permanent alpha pins. The retention webhook denies deletion/unpinning; no automatic release/blob GC or explicit artifact purge is implemented.
- **Uninstall:** removes owned disposable serving/configuration with UID/resourceVersion guards and keeps PVCs, CRDs, Images, and ImageReleases. It never deletes remote content.

Do not force-remove finalizers or delete the namespace/backend as ordinary cleanup. Administrator bypass, forced deletion, namespace deletion, and storage-provider loss are outside the retention guarantee. Recovery cannot repair a lost PVC or unavailable Kubernetes control plane.

Schema revision 4 supports only the shipped additive transitions from exact revisions 2/3, preserving CRD identities and the served/storage API version. Foreign definitions, incompatible owners, arbitrary conversion, storage migration, and schema downgrade are refused.

## Credentials and renewal

| Directory | Required files and identity |
| --- | --- |
| Registry | `ca.crt`, `tls.crt`, `tls.key`, distinct `reader-password` and `publisher-password`. Leaf SAN: `<installation>-registry.<system-namespace>.svc`. |
| Admission | `ca.crt`, `tls.crt`, `tls.key`, and at least 32 bytes in `resolution-key`. Leaf SAN: `<installation>-admission.<system-namespace>.svc`. |
| Builder | `ca.crt`, `server.crt`, `server.key`, `client.crt`, `client.key`. Server SAN: `<installation>-builder.<system-namespace>.svc`; separate server/client extended key usages. |

Use an issuer-managed server leaf, keep CA private keys outside Kubernetes, and retain current private directories for recovery. The repository's seven-day test issuer is a development fixture, not production certificate automation.

`renew-tls` takes the cluster arguments and `--admission-credentials`, without bundle arguments. It validates both registry/admission identities before either write, preserves CA/password/resolution-key material, patches owned TLS fields, and verifies the actual served certificate fingerprints. Repeat with the same issued material to resume a partial operation.

CA, password, and resolution-key rotation use a separate maintenance window: preserve the backend/releases/bundles, uninstall disposable serving, and recover with new material. Builder mTLS changes use worker reprovisioning; its cache is disposable.

## Observe and troubleshoot

Start with the resource condition, the installer phase, and normal Kubernetes events. Avoid changing storage, deleting release metadata, or bypassing TLS to work around a failed prerequisite.

```
kubectl -n apps get images,imagereleases
kubectl -n apps get image hello -o yaml
kubectl -n apps describe pod <pod-name>
kubectl -n kimage-system get pods
kimage-install <phase> --help
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

Operator/admission expose `/metrics` on existing health listeners. Proxy 0.2.0 exposes it on its loopback listener with canonical Host authority. Inspect through API port forwarding; metrics do not add an external diagnostics interface.

`kimage_stage_duration_seconds_count/sum`, `kimage_stage_active`, `kimage_stage_failures_total`, and `kimage_stage_transferred_bytes_total` cover build, publication/readback, and proxy pulls using fixed stage labels. Durations are process-local summaries without quantiles; bytes measure observed transfers, not unique storage or guaranteed client consumption.

Check `kimage_reconcile_waiting`, `kimage_images_pending`, `kimage_images_ready`, and `kimage_releases_verified`. Interpret catalog counts with `kimage_catalog_sample_complete`/`kimage_catalog_sample_timestamp_seconds`; incomplete samples retain earlier counts. Admission node counts use `kimage_nodes_eligible` and `kimage_node_sample_timestamp_seconds`. Labels exclude image names, UIDs, digests, input contents, and credentials.

**Native client API access overrides**

For an independently revalidated API access path, set both `KIMAGE_API_SERVER=https://<address>:<port>` and `KIMAGE_TLS_SERVER_NAME=<certificate-name>`. Native clients retain kubeconfig CA/credentials and refuse insecure verification. These variables do not edit kubeconfig or node configuration.

## Architecture and trust

The operator publishes complete verified graphs before sealing immutable release metadata. Publication is not one atomic registry/API transaction: durable operation identity, topological uploads, independent readback, and guarded status writes make retries recoverable.

Admission reads a coherent catalog and approved node records; it does not build, publish, or modify cluster objects. Kubernetes patches actual Pods to digest references and eligible-node constraints. Kubelet uses the normal runtime to pull from `127.0.0.1:<port>/<namespace>/<image-uid>@sha256:...`.

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

kpack also declares images. kimage's focus is immutable versioned releases combined with a managed local registry and workload selection. Its current build adapter is narrower than a general remote-source build platform.

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

CI runs 63 Rust tests on macOS/arm64 and Linux/amd64 musl, plus formatting, Clippy, documentation/example checks, and package verification. These include parser/admission compatibility for readable colon and literal/encoded query ranges.

The separate owned two-node fixture passed installation, build/input matrices, normal uncached pulls, interruption and cold recovery, actual binary upgrade/rollback, and retained-release export. Those cluster harnesses are not ordinary CI jobs. Historical receipts predate later selector/CLI changes; they remain unchanged evidence for their recorded bundle.

For exact receipts and engineering contracts, see the repository's [requirements audit](https://github.com/inferal-oss/kimage/blob/main/docs/requirements-audit.md), [acceptance ledger](https://github.com/inferal-oss/kimage/blob/main/docs/status.md), and [release procedure](https://github.com/inferal-oss/kimage/blob/main/docs/releasing.md). Report problems through [GitHub issues](https://github.com/inferal-oss/kimage/issues). Include the runtime profile, relevant sanitized condition, and reproduction steps.

# Clean implementation brief after the architecture PoCs

The user has finished the architecture-spiking phase and asked for a clean
implementation on `main`, with experimental code isolated on the orphan branch
`codex/poc-spikes`. Treat that branch as evidence and a source of lessons, not a
production baseline to merge wholesale. `main` begins with this brief and findings;
implement deliberately from that baseline. Read applicable AGENTS.md and inspect
the repository first. All four milestones below remain in scope and ordered;
start with a cohesive, tested single-node foundation and keep an honest ledger.

Read [PoC findings](poc-findings.md) before designing. The original engineering
requirements are preserved below, with obsolete “nothing implemented/no cluster
mutations” assertions removed. This introduction and the findings supersede stale
historical statements in the archived prompt. No full milestone passed in the
PoCs. Keep architectural proof, implementation completeness and production
hardening as distinct claims.

## Established decisions and implementation guidance

- Keep the Rust control plane and native macOS/Linux installer. Kubernetes objects
  contain declarations and bounded metadata; the PVC-backed internal registry
  owns authoritative OCI content. Normal kubelet/CRI pulls deliver images.
- The API-only cached-carrier/static-launcher bootstrap worked. The carrier is an
  explicit, per-node prerequisite. Never is restricted to that bootstrap carrier;
  system/application images use internal digest references and normal pulls.
- Use the proven loopback HTTP read-proxy profile for the initial tested
  containerd baseline. The proxy binds node loopback on hostNetwork and speaks
  authenticated, CA-verified HTTPS to the registry. It exposes digest-only reads,
  not writes/tags. No node mirror, trust-store, host filesystem or runtime socket
  edits are needed. Treat runtime reachability as a separate readiness condition.
- A maintained Distribution 3.1.2 registry was packaged independently. Pin and
  inventory executable/component versions; provide one cohesive offline bundle,
  not the PoC's manual sequence of scripts and local files.
- Separate immutable release publication, coherent selector/catalog resolution,
  read-only admission, and build adapters. Share the OCI verifier/publication path
  across OCI archive imports and builder exports. Preserve original graph bytes,
  media types and roots rather than flattening or recompressing images.
- Direct submits to a dedicated approved BuildKit worker. v0.33.0 with its built-in
  Dockerfile v1.27.0 frontend worked. A pinned buildctl transport helper is allowed
  with explicit argument arrays and no shell. Containerfile RUN executes on the
  worker, not in the controller. Keep reader placement, worker placement and
  eventual runtime placement conceptually separate.
- Inline versus mounted is a definition selector, not an execution strategy.
  Freeze the definition with selected inputs; supply it in a reserved frontend
  workspace independently of the ordinary COPY/ADD context root. Do not bake
  input mounts or build secrets into workload mounts.
- The PoC JSON/base64 context transport, opt-in webhook labels, shared registry
  credential, privileged worker and serial polling are experimental choices.
  Replace or explicitly scope them where the actual acceptance requirements need
  stronger behavior. Do not mistake these shortcuts for established product API.
- Sealed releases must remain usable without their original inputs, worker/cache
  or owning Image. Separate logical input identity from execution attempts/cache
  placement, and distinguish immutable-version conflicts from idempotent retries.
  The PoC conservatively rejected changed request declarations for a published
  version and did not watch referenced object mutation; complete the specified
  frozen-object identity and cache rules rather than carrying that limitation
  forward silently.
- Published readiness means a complete verified registry graph. Admission only
  resolves verified releases, never waits for/builds content, and dry-run remains
  side-effect-free. Replace bypassable PoC routing with the declared product
  policy; avoid a dependency cycle for system/recovery workloads.
- Plan durable operation identity, stale-generation checks, cancellation and
  restart recovery before adding many reconcilers. A dropped buildctl process is
  not a completed worker-cancellation protocol. Preserve retained artifacts across
  interrupted publication, cache eviction and ordinary uninstall.

## Verification and scope discipline

Use exact supported upstream APIs and selected pinned versions. The real
single-node composition is feasible, so advance to implementation instead of
repeating exploratory endpoint research. Revalidate only where the new code or
selected versions change assumptions. Keep meaningful unit checks and actual
API/registry/BuildKit/runtime tests; mocks do not replace interoperability evidence.

Known unproven requirements need explicit work: resumable install/cleanup,
complete bundle/default builder provisioning, independent read/write identities,
credential lifecycle, coherent mutable ConfigMap/Secret/PVC snapshot semantics,
Secret build sessions/definitions, PVC/subPath builds, in-flight cancellation and
recovery, publication interruption/concurrency, all-components-down recovery with
component images absent, disconnected operation, multi-node endpoint eligibility,
compatible CRD upgrades, self-upgrades and external export. The original gates
below define completion; do not claim that ten successful Pod checks close them.

Use uniquely named/labelled resources in the authorized `dev-yrashk` namespace.
Do not delete retained PoC PVC/releases/CRDs or modify shared node configuration,
policies or unrelated workloads. Use isolated test fixtures for runtime cache
absence, disconnected networking and incompatible schema tests that cannot be
proved safely on the shared cluster. Preserve TLS validation; never print tokens,
full kubeconfigs or Secret payloads. Cluster reachability problems are access-path
facts, not a reason to report unimplemented acceptance criteria as completed.

Do not build speculative extra formats, distributed schedulers or broad
production frameworks just to demonstrate extensibility. Implement the concrete
contracts and acceptance gates in order. Produce a concise initial review and
then continue with code and verification without asking for routine permission.

---

# kimage Core Engine Engineering Prompt

## Objective

Design and implement kimage, a Kubernetes extension that makes image production, publication, version selection, and node delivery declarative. Implement the core operator/control plane and native installer in Rust. A maintained separately packaged registry implementation is allowed. Preserve standard OCI images and the existing kubelet/runtime execution path.

The product goal is operation without an external registry, including disconnected operation after explicit bootstrap provisioning. kimage embeds/packages its own OCI registry backed by a PVC. Kubernetes API objects carry declarations and bounded metadata; the internal registry holds authoritative OCI image content. Existing kubelet/CRI runtimes perform normal pulls, unpacking, and node caching. Direct runtime injection is outside the baseline.

Do not claim zero dependencies, credential-free operation, automatic build reproducibility, capability-based workload selection, or immunity to tampering. Identify the prerequisites and trust boundaries that each implemented guarantee depends on.

## Working method and scope

First inspect the repository, applicable instructions, existing APIs, tests, and deployment targets. Preserve established conventions. Do not assume the proposed filenames match the repository.

Before implementation, produce a concise architectural review covering correctness, API feasibility, security, persistence, and operational recovery. For each material problem, state its consequence, the proposed correction, and how the correction will be verified. Review all requirements; do not stop after finding a minimum number of issues.

Separate confirmed upstream behavior, project design decisions, and unresolved assumptions. Verify unfamiliar containerd, CRI, BuildKit, Kubernetes, and Rust APIs against upstream documentation or source for the selected versions. Record exact supported versions and required runtime plugins. Do not invent client methods or treat a Go convenience method as an available gRPC endpoint.

Record justified amendments in the design and acceptance criteria. Do not silently expand the project or rewrite its goals. Resolve reversible implementation choices autonomously. Ask only when essential missing information changes a non-reversible choice; continue independent work meanwhile.

Implement in these milestones:

1. API-only installation of kimage and its PVC-backed internal registry, then a complete OCI publication-to-normal-kubelet-pull path on one node.
2. Format-independent build contracts, the simplest Direct execution strategy with Containerfile/BuildKit support, and immutable release publication on that single-node path.
3. Multi-node normal pulls, endpoint readiness, cache eviction recovery, and safe self-upgrades.
4. Optional OCI registry exports of retained published releases.

Complete milestones in order and report exactly which acceptance criteria passed. A scaffold, mocked import, or untested API integration is not a completed milestone. Registry export is in scope as milestone 4; importing external registry sources, cryptographic signing policy, and multi-platform builds remain deferred unless explicitly added. Design extension boundaries for them without implementing speculative general frameworks.

## Supported baseline and prerequisites

Initially support Linux nodes, one explicitly selected architecture, standard OCI Distribution and CRI behavior, and a tested Kubernetes/containerd version. Containerd is the initial interoperability target, not a private import API dependency. The client/installer may run natively on macOS; actual image execution remains in Linux nodes or VMs.

Ship the operator, registry implementation, default builder/frontend, optional registry proxy, bootstrap executables, manifests, and image bundle. The initial install must not require any kimage image to be publicly published or preloaded. Existing cluster sandbox/CNI/storage prerequisites and at least one usable cached bootstrap carrier are explicit prerequisites, not kimage's own delivered images.

Require a suitable StorageClass or explicitly selected existing PVC. Start with one registry instance and one PVC; describe capacity, access mode, attachment topology, backup, and failure limits. Do not infer HA from a PVC. Do not install a storage provider implicitly.

No external registry, public Internet, service mesh, third-party policy engine, SSH, runtime-socket mounting, node trust-store changes, or host configuration edits are prerequisites for the default profile. Internal networking, registry endpoint access, Kubernetes permissions, and persistent storage are dependencies. If endpoint/bootstrap feasibility fails, report the exact prerequisite rather than silently weakening this contract.

## API-only installation and self-hosting

Use native `kimage-install` on macOS or Linux with an admin kubeconfig and a local trusted offline bundle. The installer creates ordinary Kubernetes resources and transfers data through API-mediated streams/port-forwarding. It does not log into nodes or inject images into their runtime stores.

### Bootstrap without publishing a registry image

First prove the following composition on a real supported cluster. It is a feasibility hypothesis until tested:

1. Create a temporary bootstrap Pod using an explicitly approved image already cached on its selected node with `imagePullPolicy: Never`. The sandbox/pause image or another existing image is a candidate, not a universally discoverable guarantee. Do not alter existing workloads.
2. Mount an architecture-matched static launcher from ConfigMap binaryData with executable file permissions and override the container command. A small launcher can assemble bounded helper/registry-executable chunks into emptyDir and execute them without a shell, dynamic loader, or carrier-image tools. Respect encoded object/request limits. Only bootstrap executable chunks go into ConfigMaps; image archives do not.
3. Run the packaged registry executable with its filesystem backend mounted on the selected PVC. The bootstrap registry can therefore start before its own container image exists in any registry. Supply its configuration and credentials through Kubernetes resources.
4. Use an authenticated Kubernetes port-forward/API stream to upload the bundle's standard OCI blobs/manifests through the registry's supported upload API. Verify original descriptor bytes/digests and repository references. Do not directly manipulate an undocumented registry filesystem layout.
5. Establish the tested node-facing registry endpoint described below. If node-local proxies are needed, start their initial instances with the same cached-carrier/static-executable mechanism. They require host networking, not runtime-socket or host-filesystem access.
6. Start the ordinary pinned-image operator, registry, builder, and proxy workloads through normal kubelet pulls from the embedded registry. The operator provisions CRDs. Hand over the PVC/endpoint without simultaneous incompatible writers or loss of service; start with a documented single-writer deployment/transition.
7. Remove temporary bootstrap Pods/chunks/permissions only after ordinary workloads are healthy and recovery has been verified. Cleanup and interrupted installation are idempotent and resumable.

The bootstrap Pod must not need a kimage/helper OCI image, a public registry, runtime CLI, injected image, or an existing kimage API. Test execution of the projected static binary, actual offline carrier availability, PVC scheduling, encoded payload size, API binary transfer, and the registry-to-runtime pull path separately. A completely cold/unusable node with no allowed carrier cannot bootstrap itself through Pod creation alone; diagnose that case.

Preflight actual RBAC, namespace admission, Pod exec/port-forward, cached carrier availability, PVC access, endpoint permissions, architecture, and runtime behavior. Admin RBAC does not automatically override admission policies. Do not modify unrelated shared policies or silently fall back to SSH/insecure-registry node configuration. If an alternative node-access profile is later added, identify it explicitly as outside the default profile.

Ship native macOS/Linux `kimage-install`, matching Linux static bootstrap launchers, the registry executable/configuration, component OCI archives, and a machine-readable inventory. Authenticate package origin through an existing trusted delivery channel or signatures checked against an independently provisioned trust anchor. The inventory alone does not establish origin. Generate webhook/internal-service credentials locally or accept explicitly configured issuers; no unbootstrapped certificate-manager image may be required.

### Independent recovery and self-upgrade

Retain current and previous trusted bundles independently of the running operator/registry. `kimage-install recover --bundle <path>` can recreate the cached-carrier registry/proxy processes through the API when kimage workloads are all stopped, as long as the Kubernetes API/kubelet/carrier/PVC remain usable. Do not claim API recovery from an unusable node or lost PVC data. No host recovery service is installed by default.

System workloads use explicit internal-registry digest references and an administrator-controlled namespace excluded from their own image-resolution webhook. Their image lifecycle shares the normal publication machinery without introducing an admission dependency cycle.

Stage and verify new component artifacts before changing system workloads. When the registry or loopback proxy is needed to pull its own replacement, use ordinary node-targeted warmup Pods to pull replacement images while the old serving processes are healthy. Plan rollout ordering, endpoint handover, and carrier-based repair for interruptions; an image already running on one node does not guarantee availability on another. Do not replace a proxy that is the sole source for an uncached replacement and then expect it to pull itself.

Use a bounded compatible rollout, readiness checks, digest references, retained bundles, and explicit rollback limits. Registry/PVC schema migrations and CRD stored-data migrations are separate from binary image rollback. Never downgrade incompatible persisted data silently. Ordinary uninstall leaves CRDs, releases, and the PVC; destructive purge is an explicit distinct operation.

## Component boundaries

- Operator/controller: provisions/upgrades CRDs, reconciles builds and immutable releases, and manages internal registry/proxy configuration and readiness.
- Build subsystem: separates definition format, execution strategy, builder identity/placement, and cache policy. Initially use an isolated BuildKit worker through the Direct strategy.
- Embedded registry: standard OCI Distribution endpoint backed by a PVC, authoritative for retained image content. Embed/package a proven implementation first; do not casually invent a partial registry protocol.
- Admission service: read-only HTTPS axum service resolving published releases and returning digest-pinned mutations; never builds or transfers artifacts during admission.
- Optional node-local registry proxy: read-only pull gateway to the central registry, used only if needed for a node-reachable endpoint without host configuration changes. It does not touch the runtime socket or manually populate node image stores.
- External exporter: optional independent reconciler copying retained releases to user-configured external registries.

Use separate least-privilege identities. Builders, proxies, admission, and exporters have no runtime socket access. Restrict registry writes to approved publication/installer identities; scope internal transport authentication and registry read policy explicitly.

## Operator-owned CRD lifecycle

The operator is the authoritative manager for provisioning and upgrading the `kimage.dev` CRDs, including `images.kimage.dev` and `imagereleases.kimage.dev`. Initial installation and supported operator upgrades must not require a separate `kubectl apply` of CRD files, a Helm CRD-upgrade step, or an external schema-management service. Ship versioned CRD definitions with each operator release and make them available offline.

Start the operator from ordinary Kubernetes resources and already seeded images, with no existing kimage custom resources required. Its first startup phase uses built-in `apiextensions.k8s.io`, `coordination.k8s.io`, and core APIs to coordinate leadership, inspect/manage schemas, and report progress. Only start custom-resource discovery, typed watches, reconciliation, and normal managed-image admission once the required CRDs are established and compatible. A webhook configuration must not become active before its serving endpoint and required schemas are ready. Liveness must tolerate a pending or failed schema phase without a crash loop; readiness and a built-in ConfigMap/log/event diagnostic expose the actual failure.

Use a built-in Lease for a single CRD manager across replicas. Persist schema revision, definition digest, managed-field ownership, and migration progress on the CRDs or built-in checkpoint resources so restart can resume without requiring a new custom API first. A Lease alone does not prevent an older binary from later taking leadership: each binary must inspect installed revision and reject unsafe backward changes before any write. Define compatibility by supported schemas and migration paths, not by merely comparing product version strings.

Generate schemas from a version-controlled source of truth and verify that the embedded definitions match Rust types, validation, and release metadata. Reconcile only explicitly owned CRDs and owned fields, with optimistic concurrency or a dedicated server-side-apply field manager. If a conflicting foreign definition/field owner is present, report an ownership conflict rather than force-overwriting or deleting it. Kubernetes system controllers still own normal CRD status conditions; do not replay their status wholesale.

Grant the operator explicit cluster-scoped CRD permissions. Restrict reads/updates/patches to the known CRD names where RBAC supports it, avoid unrelated-resource wildcards, and document that top-level CREATE cannot be restricted by `resourceNames`. Keep application builders/exporters/registry proxies free of CRD-management permissions. Do not attach deletion-coupled ownerReferences to the operator Deployment or namespace: managing a schema must not cause its resources to disappear when an operator Pod or installation is removed.

Wait for `Established=True`, valid name acceptance, and usable discovery before reporting schema readiness. Installation/upgrades must be idempotent and resumable after interruption. Do not implement upgrades by deleting and recreating CRDs: deleting a CRD destroys its custom-resource instances. Preserve existing object identities, specs, statuses, finalizers, release references, and recorded digests.

For alpha, implement initial provisioning, same-version reconciliation, additive compatible schema upgrades, revision/ownership-conflict detection, and refusal of unsupported incompatible transitions. For future API-version changes, require an explicit operator-managed migration path:

1. Prepare code able to read the existing schema and the new schema. Bring up any necessary conversion endpoint and its TLS credentials before CRDs reference it; this endpoint must start without depending on custom-resource watches.
2. Expand the CRD to serve compatible old/new versions and configure tested conversion only when required. Do not use `conversion: None` for a structural rename that needs data conversion. Keep exactly one storage version.
3. Switch storage version in a documented sequence and migrate existing objects with the supported Kubernetes storage migrator when available, or a tested resumable operator-owned rewrite procedure. Changing `storage: true` alone does not rewrite existing stored objects.
4. Verify migration and compatibility before retiring old served versions or conversion support. Update `status.storedVersions` only when old storage has actually been eliminated; never clear it merely to bypass API validation. Preserve old API access throughout the supported compatibility window.

Pause/refuse a transition that lacks a safe implemented migration, with an actionable schema condition and diagnostics. Image rollback must never apply an older incompatible CRD definition over migrated data. Preserve compatible expanded schemas across a binary rollback; reject a binary that cannot read the installed API. Keep ordinary uninstall separate from an explicitly destructive purge: stopping/removing the operator leaves CRDs, release metadata, and retained artifacts in place.

## Resource and release model

Use `kimage.dev` as the public project domain and Kubernetes API group. Use `https://kimage.dev` for the project website/documentation and `kimage.dev/<key>` for project-owned annotations, labels, and finalizers. API group and metadata prefixes are identifiers; operation without an external registry and disconnected operation must not depend on resolving or contacting that domain. Keep `platform://` as the symbolic image-selector scheme. Resolve it to the actual tested internal-registry endpoint, not a fictitious local-cache registry prefix.

Keep `kimage` as the project name and `kimage-install` as the native installer executable name. Use `Image` as the public resource kind and `ImageRelease` for published releases. Do not require a per-image/per-node availability CRD for ordinary registry pulls. The API group supplies the resource identity; avoid adding a product prefix to every kind. Document fully qualified resource names such as `images.kimage.dev` when command-line disambiguation is needed.

Use namespaced `Image` resources in `kimage.dev/v1alpha1` for desired state. Use separate namespaced `ImageRelease` resources for immutable published versions; do not maintain an unbounded `status.versions` table on one object.

A published release records the originating Image UID, exact semantic version, source snapshot identity, OCI root descriptor, selected platform, builder identity, and publication outcome. Store no archive bytes or raw secret values in API objects. Distinguish an image manifest digest from an image index digest, layer digest, config digest, and archive-file checksum.

Enforce these rules:

- Version identity is scoped to the Image UID. Deleting and recreating an Image with the same name creates a different identity.
- Published release identity and content are immutable. Repeated publication of the same version and content is idempotent; different content for an already published version fails with `VersionConflict`.
- For alpha, reject version build metadata (`+...`) to avoid equal-precedence ambiguity. Support prerelease versions explicitly.
- Channels are explicit spec mappings from channel name to exact published version. There is no implicit `latest` or inferred approval semantics.
- A release is selectable only after artifact verification and complete durable publication in the internal registry. Existence in any particular node cache is not an admission prerequisite.
- A build that finishes for obsolete inputs or an obsolete generation must not overwrite current status or publish as the current requested result.

Use status subresources, optimistic concurrency, `observedGeneration`, conditions with reason/message/transition time, and retry-safe reconciliation. Controller restart must rediscover work without treating an in-memory task as authoritative. Specify retry/backoff, cancellation, permanent errors, stale acknowledgments, and concurrent publication behavior.

If a loopback-proxy profile is selected, track bounded per-node endpoint readiness independently of image digests. A standard node label/status projection may suffice; do not introduce a per-release cache table or acknowledge manual import.

## Source schema

Exactly one source variant must be present. Enforce this in both Rust deserialization and generated structural CRD validation. Reject malformed combinations at API validation time.

Use `source.build` for a build declaration and `source.ociArchive` for a prebuilt OCI import. Do not encode Containerfile or inline text as the top-level build mode: inline versus mounted is a definition-source choice, and Containerfile is one build format.

A build declares named input volumes and mount mappings on the image resource itself. Reuse familiar Kubernetes `volumes` and `volumeMounts` field shapes for a validated subset, rather than requiring a separate hand-written builder Pod. Distinguish the input-reader workspace, the Containerfile build context, individual build-step mounts, and runtime workload mounts.

An initial inline example is:

```yaml
apiVersion: kimage.dev/v1alpha1
kind: Image
metadata:
  name: api-service
  namespace: core-apps
spec:
  version: "1.2.4"
  channels:
    stable: "1.2.4"
  source:
    build:
      format: Containerfile
      definition:
        inline: |
          FROM scratch
          COPY app/server /app/server
          COPY config/application.yaml /app/application.yaml
          ENTRYPOINT ["/app/server"]
      inputs:
        contextPath: /workspace/context
        volumes:
          - name: payload
            configMap:
              name: backend-payload
              items:
                - key: server
                  path: server
                  mode: 0555
          - name: configuration
            configMap:
              name: backend-build-config
              items:
                - key: application.yaml
                  path: application.yaml
          - name: source-tree
            persistentVolumeClaim:
              claimName: backend-source
              readOnly: true
          - name: signing-input
            secret:
              secretName: backend-build-secrets
              items:
                - key: license
                  path: license
              defaultMode: 0400
        volumeMounts:
          - name: payload
            mountPath: /workspace/context/app
            readOnly: true
          - name: configuration
            mountPath: /workspace/context/config
            readOnly: true
          - name: source-tree
            mountPath: /workspace/context/src
            subPath: source
            readOnly: true
          - name: signing-input
            mountPath: /run/kimage-build-secrets/signing
            readOnly: true
      buildSecrets:
        - id: license
          sourceFile: /run/kimage-build-secrets/signing/license
      networkMode: none
      execution:
        strategy: Direct
        builderRef: default
      cache:
        mode: Local
```

The example requires a statically linked executable suitable for the target platform, supplied through ConfigMap `binaryData` only when it fits Kubernetes object limits. It does not imply an arbitrary application or archive fits in a ConfigMap. The source PVC illustrates mapping a larger source tree for compiler-based Containerfiles; the scratch fixture only copies the prebuilt executable and selected configuration. The secret mapping binds an available build secret; a Containerfile must explicitly request it to consume it.

### Containerfile from any supported mounted input

For `source.build.format: Containerfile`, the build definition does not have to be literal inline text. Make `source.build.definition` a reusable source selector with exactly one of:

```yaml
definition:
  inline: |
    FROM scratch
    COPY app/server /app/server
    ENTRYPOINT ["/app/server"]
```

or:

```yaml
definition:
  fromFile:
    path: /workspace/context/src/Containerfile
```

The second form reads the file from the `source-tree` mount in the full example. The same mechanism must work for a ConfigMap item, a PVC file, or another approved input-volume source; do not implement separate Containerfile fetch logic for every storage backend. A Secret may also supply a build definition from its approved mount outside the ordinary context, subject to the same access policy and sensitive-data handling. A future volume adapter can supply the file without changing the Containerfile selector.

Resolve `fromFile.path` only through declared input mounts inside approved workspace roots. It must identify a bounded, regular text file, not an arbitrary path on the controller/node/BuildKit host. Reject missing files, directories, path traversal, symlink escapes, and simultaneous inline/fromFile selectors. Validate supported encoding and size limits before submitting the build.

Read and freeze the mounted Containerfile with the ordinary input snapshot before launching the solve; never reread a changing file between build stages. Feed the frozen file through the Dockerfile frontend's build-definition input/session. Keep that input distinct from the context root: locating the Containerfile under `src/` does not change the meaning of `COPY app/server`. COPY/ADD source paths remain relative to `inputs.contextPath` unless an explicitly named context is requested.

Materialize a reserved copy for the frontend when necessary, without overwriting the source mount. Record its input identity and include its contents in the appropriate private build fingerprint. Containerfile content changes invalidate build work/cache selection under the immutable-version rules. Keep Secret-sourced definition bytes and sensitive fingerprints out of ordinary logs/status/provenance. Supplying a Containerfile via a Secret does not make literals used by that Containerfile safe from image history, cache, or deliberate build output; credentials still belong in build-secret mounts.

Define these input and mount semantics:

- Allow ConfigMap, Secret, and same-namespace PVC volume sources initially. Each named volume must have exactly one supported source. Validate unique names, existing references and selected keys, key-to-path mappings, file permissions, binary data, and mount-name resolution. Reject unsupported volume kinds explicitly. Do not accept arbitrary Kubernetes Pod templates or permit privilege escalation through a volume declaration.
- Mount inputs in a dedicated least-privilege input-reader/session process. Generate the necessary Pod/Job mappings from the Image declaration. Do not grant the image author access to the controller filesystem, runtime socket, or arbitrary BuildKit host paths.
- `inputs.contextPath` is the root of the assembled ordinary context. `mountPath` is an absolute path within the controlled input-reader workspace. ConfigMap/PVC paths beneath the root map directly to relative Containerfile COPY/ADD sources: `/workspace/context/src/main.rs` becomes `src/main.rs`, not `/src/main.rs` in the final image. Restrict absolute mount destinations to approved workspace roots.
- Secret volumes mount beneath a dedicated memory-backed secret-input root outside the ordinary context. `buildSecrets[].sourceFile` must resolve to an explicitly mounted Secret item. Bind it to the BuildKit secret session by ID; it is not an ordinary local-context file. Permit a direct same-namespace `secretKeyRef` alternative only if it has identical authorization, snapshot, and secret-session semantics. Reject Secret mounts into the ordinary COPY/ADD context in the baseline.
- Support read-only directory and single-file mounts with explicit `subPath`. Validate that subPaths remain beneath their source roots. Reject `subPathExpr`, mount propagation, writable source mounts, ambiguous nested/overlapping destinations, workspace-control-file replacement, and missing required input paths initially. Specify deterministic error behavior rather than depending on incidental Kubernetes overlay ordering.
- Generate or snapshot the selected Containerfile into a reserved frontend workspace location and prevent input mounts from overwriting that staged copy. A mounted source file is read as input, never modified. User-configured mounts describe build inputs; they are not inherited by the final application Pods. Runtime ConfigMap/Secret/PVC mounts continue to be declared in the workload's Pod specification.

Mounting files into the input-reader or builder Pod does not make them appear automatically inside a Containerfile RUN instruction. Expose ordinary files through COPY/ADD or BuildKit context bind mounts, and secrets through explicit build-step secret mounts. For a Containerfile stage with the appropriate provisioned toolchain, the mapping above permits a step such as:

```dockerfile
RUN --mount=type=bind,source=src,target=/src,ro \
    --mount=type=secret,id=license,target=/run/secrets/license,required=true \
    /usr/local/bin/compiler --source /src --license /run/secrets/license --output /out/server
```

The compiler path is illustrative and must exist in the selected toolchain stage. This RUN example is not executable in the scratch-only fixture. Define and test how the pinned frontend resolves context bind paths. No implicit live host mount may cross into the BuildKit execution sandbox.

Support an import variant such as `source.ociArchive`, referencing a same-namespace PVC, a relative path beneath its approved root, and an expected OCI root descriptor. It must contain a complete image, not a raw binary. Persist verified content before publishing a release; do not depend on the continued presence of a transient source file.

Reserve a separate external-source variant for a later migration profile. Reject it as unsupported in the initial baseline rather than silently reaching the Internet.

Context item paths and Containerfile input paths are relative to the build-context root, not paths on a developer laptop or inside the final image. Define allowed sizes and deterministic traversal. Reject absolute item/subPaths, `..`, duplicate destinations, symlink escapes, and oversized inputs. Absolute mountPath is permitted only within approved workspace roots. Host-path access and arbitrary local directory mounts are outside the baseline.

Snapshot referenced ConfigMaps and Secrets once per build and record their identities/resource versions. Capture each selected object version explicitly; mounted Kubernetes projections can update during a build and do not supply a reproducible cross-object snapshot. Feed frozen ordinary files to the context session and keep secret snapshots in memory-backed storage. Preserve expected binary contents and file modes when snapshotting. Do not rely on subPath projection behavior as a snapshot protocol.

For PVC sources, record the claim/volume identity, selected subtree, and the digest of the frozen ordinary input snapshot. A read-only consumer mount does not prevent another writer from changing the source. Require a documented producer handoff/quiescence contract or a supported storage snapshot when consistent PVC input is needed; copy from that stable input before building. Respect PVC access modes, attachment topology, and source-node affinity when scheduling the input-reader/build Job. Do not claim an atomic filesystem snapshot from a plain recursive copy.

Changes require a new build identity; different output requires a new release version. Changes to selected ConfigMap keys, mounts, file modes, context paths, or ordinary PVC bytes must invalidate the ordinary context cache. Prefer explicit key selection over inclusion/exclusion glob features initially.

Secrets use a separate `buildSecrets` mapping to BuildKit secret mounts. Never copy them into the ordinary context, arguments, logs, status, or provenance. Document that build commands can deliberately read and disclose mounted secrets; secret mounts do not make untrusted build instructions safe. Define authorized access and a non-sensitive explicit cache-invalidation token when secret-dependent builds need it; do not assume secret values automatically invalidate build cache.

## URI grammar and version selection

Implement a custom parser with a documented grammar for:

- Exact: `platform://core-apps/api-service:1.2.4`
- Range: `platform://core-apps/api-service:^1.2.0`
- Range: `platform://core-apps/api-service:~1.2.0`
- Query range (also supported): `platform://core-apps/api-service?version=^1.2.0`
- Channel: `platform://core-apps/api-service:stable`

Namespace and name must satisfy Kubernetes naming rules. The colon selector is a complete exact semantic version, a validated channel name, or a range beginning with an explicit operator (`^`, `~`, `>`, `<`, `=`) or `*`. A numeric-looking invalid version must not fall back to a channel or range. Reserve numeric-leading channel names to eliminate ambiguity.

Both colon and query forms accept ranges. Prefer readable literal syntax in manifests. The query form also accepts percent encoding, decoded once; colon ranges use literal characters. Reject fragments, userinfo, ports, extra path segments, duplicate/unknown query parameters, malformed encoding, and conflicting colon/query selectors. A generic URL library alone does not define this custom grammar.

Parse exact selectors with `semver::Version`, not a bare `VersionReq` that can interpret `1.2.4` as a compatible range. For ranges, use the pinned `semver` crate's documented requirement syntax and prerelease matching rules, then choose the highest eligible version by semantic precedence. Do not sort strings lexicographically. Treat channels as exact mappings. No match is an explicit admission denial.

Resolve against a coherent release/catalog snapshot and attach the selected release UID, exact version, digest, original selector, and relevant resource version to bounded Pod metadata. Reject forged or inconsistent resolution metadata; user annotations are not trusted authorization evidence.

Default to same-namespace references. Cross-namespace consumption is denied until an explicit publisher/consumer grant model is implemented and tested. The controller's permission to read a release does not authorize every workload to consume it.

## Admission and rollout behavior

Mutate actual Pods on CREATE. Leave controller templates symbolic, so a Deployment, StatefulSet, Job, or other controller's generated Pods are resolved through the same path. An admitted Pod's selected image remains pinned. Later channel/range changes affect newly created Pods, including replacement replicas, and can therefore produce mixed versions. Do not automatically restart workloads. Explain this behavior and recommend exact versions for uniform rollouts.

Handle regular and init containers, including restartable init sidecars. Explicitly reject platform references introduced by unsupported Pod image updates or the ephemeral-container subresource until those paths have dedicated handling. Mirror/static Pods are outside the baseline; do not claim that mutating a mirror Pod changes its underlying static Pod.

Emit an immutable registry reference using the selected node-facing endpoint and the published root digest. For a proven loopback-proxy profile an example is `127.0.0.1:<configured-port>/<namespace>/<image-uid>@sha256:<64-lowercase-hex>`; for a direct trusted-TLS profile use the configured node-reachable registry authority. The repository identity must not collide after Image recreation. Never use `<name>:sha256:<hex>` as an image tag.

Use `imagePullPolicy: IfNotPresent` for managed digest references: normal kubelet pulls from the internal registry when the local cache is missing. Do not inject `Never` into ordinary managed workloads; it is reserved for the initial cached-carrier bootstrap. No pull path may silently fall back to a public/external registry.

Return `admission.k8s.io/v1` AdmissionReview with the request UID, `allowed`, structured denial information, `patchType: JSONPatch`, and a base64-encoded RFC 6902 patch when mutation is required. Use `add` for missing members and `replace` for existing members, escape JSON Pointer paths, and set managed containers' `imagePullPolicy` to `IfNotPresent`. Preserve unrelated fields and unmanaged images.

The webhook is read-only, bounded in time, idempotent under reinvocation, and safe on `dryRun`. It does not wait for a build or long distribution operation. Deny unresolved or unavailable managed references with actionable reasons. Unmanaged external OCI references pass unchanged when the handler is available. Apply documented consumption policy to direct internal-registry references as well as symbolic references; a raw registry URI must not be assumed to prove authorization.

Deploy valid TLS serving certificates, a CA bundle, renewal/reload, readiness checks, bounded request bodies, graceful shutdown, and suitable replicas. Use `sideEffects: None` and a documented timeout/failure policy. Prefer fail-closed handling for the declared managed scope. Explain whether webhook outage blocks ordinary Pod creation in that scope; do not claim selective failure behavior unless webhook routing actually provides it. Exclude bootstrap system workloads through an administrator-controlled installation scope.

## Internal registry and runtime delivery

The baseline uses normal OCI registry pulls. No native containerd archive importer, image registration, snapshotter unpack implementation, runtime namespace management, runtime socket mount, or runtime CLI is required in production.

Use a proven OCI Distribution implementation with a filesystem backend on a single PVC initially. Pin its version, package the standalone executable/images for bootstrap, and test the features used by kimage: blob upload/read/HEAD, manifest upload/read/HEAD, digest references, repository naming, interrupted upload behavior, appropriate content types, and any Range requests used by the tested runtimes. If implementing an embedded Rust server instead, make this a deliberate measured choice and prove protocol compatibility before replacing a maintained implementation.

Validate incoming OCI layouts/archives before publication: root descriptor, referenced manifests/config/layers, sizes, SHA-256 digests, platform, and supported media types. Distinguish manifest/index/config/layer/archive digests and uncompressed DiffIDs. Preserve original manifest/blob bytes, layer order, and compressed-layer semantics. The archive is a transfer format; the registry serves OCI objects, and the runtime handles unpacking. Do not flatten/recompress/regenerate content during import or export.

Stream with bounded buffers, backpressure, deadlines, cancellation, and limited concurrency. Disk spooling is acceptable. Upload blobs before the root manifest, verify the registry content and digest, and only then publish the selectable ImageRelease. Cross-object registry/API publication is not an atomic database transaction: use idempotent reconciliation and recovery so partial artifacts never become a falsely Ready release.

Keep the registry's filesystem/metadata consistent across restart, failed uploads, registry replacement, and PVC handover. Do not write archive contents directly into implementation-specific registry directories. Backup/restore must include the entire registry backend and corresponding release metadata. A durable image release is separate from disposable builder and runtime caches.

### Node reachability and TLS without node edits

Prove an endpoint reachable by the node's real kubelet/runtime, not just from an application Pod. Node DNS does not automatically have Pod DNS behavior, and a registry Service alone does not establish runtime TLS trust. A direct endpoint is acceptable when node-level DNS/reachability and a trusted certificate already work without installing a CA or changing runtime configuration.

Evaluate a node-local loopback profile first when those direct prerequisites are absent: a hostNetwork DaemonSet binds a read-only registry proxy to loopback on a configured nonprivileged port, forwarding to the central registry through authenticated internal transport. Use `ClusterFirstWithHostNet` where needed for proxy-side service DNS. The node runtime sees `127.0.0.1:<port>`, while the proxy's own client can receive private-CA trust and credentials through Pod resources rather than changing host trust stores.

Containerd has version-dependent special localhost resolution/HTTP fallback behavior; verify it in the pinned CRI pull path, not only with ctr or a generic HTTP client. Require a real ordinary Pod pull with untouched node configuration. Do not treat localhost behavior as portable to every runtime or assume a self-signed certificate is trusted. If the profile does not work, report the limitation and select a supported trusted endpoint; do not silently write hosts.toml, restart containerd, disable remote TLS checks, or open plain HTTP on a routable node interface.

The proxy exposes only read operations for approved internal repositories; it is not a public write registry, arbitrary forward proxy, or upstream Internet fallback. Bind only loopback, detect port conflicts, configure finite buffers/timeouts, and authenticate/verify its central-registry connection. Define auth challenge/token behavior so clients are not redirected to unreachable private endpoints. Every eligible node needs a ready proxy in this profile, but it does not need the particular image preloaded.

The administrator-managed baseline may trust eligible nodes as readers; state that explicitly. Namespace selector policy alone does not establish image confidentiality between hostile tenants on a shared node. If stronger consumption isolation is implemented, enforce the registry/proxy policy and manage any required pull credentials declaratively. Users must not manually maintain registry credentials for the default internal path.

This loopback design is a feasibility candidate, not an already validated default. Choose and record one working endpoint profile based on real-cluster evidence before building broad scheduling abstractions.

## Build formats, execution strategies, builders, and caches

Keep these choices independent in the public API and implementation:

| Choice | Meaning | Initial implementation |
| --- | --- | --- |
| Format | Language/structure of the build definition | `Containerfile` |
| Definition source | Where the recipe comes from | Inline text or a declared mounted file |
| Execution strategy | How work is submitted, tracked, cancelled, and recovered | `Direct` |
| Builder | Approved engine implementation and worker identity | `builderRef: default`, backed by pinned BuildKit |
| Placement | Where input capture and actual execution happen | Administrator-selected local worker and compatible input-reader placement |
| Cache policy | Which intermediates may be reused and retained | Worker-local cache, or reuse disabled |

Containerfile is not synonymous with BuildKit, and Direct is not a build format. Future formats may include Buildpacks, an LLB definition, a language-specific recipe, or another supported description. Future strategies may submit a Kubernetes build Job, select from a worker pool, or delegate to an authenticated build service. These names describe extension directions, not already supported features. Initially reject unsupported formats, strategies, builder capabilities, placement requests, and cache modes with precise errors.

### Simplest Direct strategy first

Implement one concrete strategy before any queue manager or distributed build scheduler. Direct submits one frozen build request to one explicitly resolved approved builder, tracks the engine operation, and collects its OCI result. The baseline default is a pinned, isolated BuildKit worker provisioned with the installation on the tested build node. It may run as a separately isolated local process or in-cluster worker; document the selected packaging and keep its endpoint inside trusted configuration.

Direct does not create a per-build execution Job, distribute build stages, perform dynamic pool selection, or reach an external CI service. The input-reader Pod/Job needed to safely mount ConfigMaps/Secrets/PVCs is a separate input-capture concern and may still be needed. Apply bounded admission/concurrency and deadlines without designing a general scheduling framework. Direct still needs operation identity, cancellation, engine failure detection, and restart recovery; it must not mean executing untrusted commands in the controller process.

Resolve `execution.builderRef` against an operator-administered builder catalog/configuration, initially an embedded/default configuration or controlled ConfigMap rather than a mandatory new CRD. The builder entry declares engine type/version, endpoint, permitted formats/platforms, execution identity/trust scope, placement, and local-cache budgets. Image authors select authorized entries; they cannot set arbitrary endpoints, host paths, service-account identities, or worker privileges. Provision the default builder and dependencies without requiring an external registry.

Separate the node that reads source volumes from the node where the engine executes. Respect PVC access/attachment and node affinity for the reader, then transfer the frozen context through the supported session/transport. Do not assume a file mounted into one Pod is present on a remote worker. In the first single-node implementation, keep these roles on the selected node and fail unsupported placement clearly. Execution placement and later image-runtime placement are also distinct.

### Narrow internal contracts

Introduce small interfaces at the existing seams, not a speculative plugin framework:

- Definition/format handling validates the selected format and its options, resolves inline/mounted inputs, and produces a frozen request suitable for the selected builder. Keep Containerfile parsing and BuildKit frontend options inside their adapters.
- Strategy handling resolves the authorized builder/placement, submits an operation, reports progress, cancels, and resumes or safely retries after interruption. Use the same strategy contract for future Job or delegated execution without requiring the release controller to know the execution mechanism.
- Builder integration implements the concrete engine protocol and capability checks. BuildKit/LLB/protobuf types remain within that integration; do not require all future formats or engines to translate through LLB.
- Result validation accepts OCI content plus an expected descriptor/platform and execution receipt, then enters the common artifact publication path. Publication, ordinary runtime pulls, version resolution, and external registry export do not depend on how the artifact was built.

Use a common request/result/operation identity and cancellation model. Record definition format and adapter version, selected builder/engine/frontend versions, actual worker identity/platform, input snapshot, cache mode, and verified OCI result. An artifact descriptor, not a builder-specific image name or task handle, is the handoff to publication. A local file/stream can carry initial results; future remote artifact handles require an authenticated resolver before verification.

Keep schema extension explicit and typed. Define a namespaced/versioned identifier convention and capability negotiation before adding another format or strategy. Do not enable arbitrary code loading, arbitrary serialized engine options, or user-selected frontend image execution through an unchecked string. Add new validated variants incrementally through the operator-owned CRD lifecycle. Do not implement a second production builder merely to demonstrate extensibility.

### Cache policy independent of artifact retention

Initially support `cache.mode: Local` and `cache.mode: Disabled`. Local reuses only the selected approved worker's existing intermediates within its configured trust scope and quotas. Disabled prohibits previous build-result reuse through the engine's tested no-cache mechanism; it does not promise the engine allocates no temporary content or caches nothing internally during one solve. Clean up intermediates under an explicit worker GC policy. Neither mode requires a registry cache or remote cache service.

Cache is disposable acceleration; retained releases and authoritative OCI blobs are separate. A cold, evicted, or corrupt cache must not lose a published image. Validate/fail a corrupt cache entry safely rather than publishing unverified output. Record effective cache policy, apply per-worker budgets, avoid unbounded cross-tenant sharing, and document the administrator-managed baseline's trust boundary. Future shared/remote cache import/export needs its own authorization, trust, transport, and lifecycle contract; it is not implied by optional image registry exports.

Keep logical recipe/input identity distinct from execution attempts. A builder placement change, retry, or cache-policy change does not by itself require rebuilding an already published release. When a missing release is built or an operation is explicitly retried, record the actual execution settings and verify the output under the immutable-version rules. Changed recipe/format options, relevant dependency identities, target platform, or frozen ordinary inputs affect the logical build identity; cached hits must still refer to the correct inputs. Different bytes for an already published version remain a conflict regardless of strategy or cache source.

### Initial Containerfile and BuildKit adapter

Use the isolated, pinned BuildKit worker selected by the initial Direct strategy. Implement the Rust control adapter against real supported APIs; account for context/secret sessions and OCI export. If a pinned `buildctl` helper is necessary for the builder milestone, record that choice and invoke it with explicit arguments without a shell. Neither the build adapter nor installer may use direct runtime injection as the publication/delivery path.

Write a bounded transient workspace, resolve the inline-or-mounted Containerfile selector, freeze it with the input snapshot, supply the frontend build-definition session, build for the declared platform, and export a standard OCI layout archive. It may use disk and layered output; neither in-memory-only builds nor flattened layers are required.

Disconnected mode must cover base-image resolution, Dockerfile frontend resolution, remote ADD/context access, package downloads, and build execution networking. `RUN --network=none` alone does not close all of these paths. Missing offline dependencies fail with precise diagnostics; never silently fetch them. First use `FROM scratch` and pre-provisioned inputs for the acceptance fixture.

Disable insecure/host-network entitlements. Apply enforceable worker isolation, CPU/memory/disk/time/concurrency limits, cleanup, and log limits. The baseline is an administrator-managed cluster, not a proven hostile multi-tenant build service. Record the isolation mechanism and its limits.

Do not infer reproducibility from a source hash. Record the input snapshot, frontend/builder versions, base-image digests when present, platform, and resulting OCI descriptor. Stable hashing is distinct from reproducible execution and from authenticated provenance. Signatures, attestations, trusted signer policy, and offline key rotation require their own later acceptance criteria.

## Multi-node delivery and scheduling

For milestone 1, run the complete registry/pull path on a selected test node and label the result single-node support. For milestone 3, demonstrate ordinary pulls from at least two real nodes using an image absent from both caches. Kubernetes/runtime own pulling, unpacking, image cache, and image GC; kimage does not eagerly replicate every image or require a per-release scheduling label.

Preserve workload nodeSelectors, affinity, tolerations, and platform constraints. For direct registry endpoints, readiness is global service/endpoint readiness plus normal platform scheduling. For a loopback profile, constrain only to nodes whose configured proxy is ready; merge this node-level requirement conjunctively into each existing required-affinity OR term. Reject incompatible explicit nodeName/placement or unsupported scheduler bypass as documented. Proxy readiness is not a per-image cache acknowledgment.

Cache deletion is recoverable through ordinary internal-registry pulls without changing the selected digest, rebuilding, or invoking a custom importer. Test rescheduling, new/replaced nodes, proxy restart, registry restart, runtime cache misses, and registry downtime. Existing running containers may continue during registry downtime; new uncached starts may fail until service recovery. Never report a single PVC registry as highly available.

Do not require existing cached carrier bootstrapping for every ordinary application; it is only for bringing up/recovering the registry/proxy path. New-node baseline sandbox/carrier provisioning remains the cluster provider's responsibility. Test proxy eligibility and self-pull rollout cycles without altering user node configuration.

## Optional registry exports

Installation, builds from provisioned inputs, internal publication, and node delivery without an external registry remain mandatory capabilities. Users may additionally export published images to explicitly configured OCI registries for use by other clusters, ordinary deployment pipelines, or customers. An external registry is an optional additional destination. The embedded PVC-backed registry remains authoritative; external push availability is not a prerequisite for internal readiness.

Declare export targets independently of the build source:

```yaml
spec:
  exports:
    - name: production
      registry:
        repository: ghcr.io/example/api-service
        tags: ["1.2.4"]
        credentialsSecretRef:
          name: registry-push-credentials
        overwritePolicy: Never
```

An export reconciles the immutable published release for the Image's requested exact `spec.version`. Bind every export attempt to that release UID and OCI root digest; never resolve its content again through a moving channel during transfer. Support exporting an already retained release without rebuilding it. Export configuration and credential changes do not change build-input identity or invalidate the release's OCI digest.

Implement an exporter adapter over the retained artifact store using the OCI Distribution protocol. BuildKit supports direct pushes, but a completed build must not require its exporter to stay alive while destinations retry. Upload existing verified blobs and the original manifest/index bytes; do not recompress, flatten, regenerate the manifest, or rebuild for a destination. The exported root digest must equal the retained published root digest. Reject a destination that cannot accept the supported media types rather than silently converting content and changing identity.

Validate unique bounded target names, explicit repositories, explicit OCI-compatible tags, and overwrite policy. No implicit destination, public push, or `latest` tag is allowed. External repository names are independent of the internal registry/proxy authority and repository identity. Tags are aliases attached to the selected immutable release, not proof of identity. Push under digest as supported by the destination and report the canonical remote `repository@sha256:...` reference.

Use `overwritePolicy: Never` by default. An observed tag at the same digest is an idempotent success; an observed tag at a different digest is `TagConflict`. Serialize kimage's own updates for the same destination/tag. Generic registry tag updates do not supply a portable compare-and-swap transaction: an external writer may race between a check and PUT. Strong tag immutability therefore requires registry-side enforcement; document this limitation and verify resulting tags after push. Permit overwriting an existing tag only through an explicit supported policy with documented ordering; do not automatically mirror moving channels in the initial export implementation.

Resolve push credentials from an authorized same-namespace Secret, preferably using the standard `kubernetes.io/dockerconfigjson` format. Match credentials to the exact configured registry authority. Permit unauthenticated pushes only when explicitly configured and accepted by that registry. Do not send push credentials into the build context, Containerfile secret session, registry proxies, admission service, logs, or status. Use destination-scoped exporter credentials, HTTPS certificate verification, and explicit trust configuration for private CAs; never silently disable TLS verification. External egress is enabled only for explicitly configured exports and is blocked in disconnected operation.

Maintain bounded per-target export status independently of build/publication/node conditions. Include export configuration generation, selected release UID, local and verified remote digest, remote reference, phase, attempt time, retry eligibility, and sanitized error reason. A failed external export must not mark an internally published release unavailable or stop locally deployed Pods. Use bounded retries/backoff, concurrency, deadlines, cancellation, and idempotent recovery after exporter restart. Credential rotation or destination recovery permits another attempt against the same retained digest.

Keep the selected artifact retained while an export is active. On target removal or Image deletion, cancel future export work and apply the documented local retention policy; do not delete remote repository content automatically. Remote registry retention, tag mutation by other actors, and deletion permissions are outside the baseline exporter contract.

Distinguish successful image export from future signature/provenance export. Do not claim supply-chain attestations were copied unless their formats and referrer behavior are explicitly supported and tested.

## Retention, deletion, and trust

Define separate retention policies for durable artifacts, published release metadata, build cache, and node caches. Protect releases referenced by admitted active Pods and retained workload/release pins. A symbolic template may request a historical version again, so define explicit retained pins and a grace period rather than pretending there is an atomic global reference count.

For alpha, retained published releases and their durable blobs are not automatically deleted. Explicit release deletion is denied while known usage/pins remain; document concurrent-admission/deletion handling and its limits. Image deletion must not cascade away artifacts required by running workloads. Use ownership/finalizers or detached retained releases consistently, and document uninstall/force-delete behavior. Runtime image-cache eviction remains recoverable through ordinary pulls while the durable release is retained and the internal endpoint is available. Registry garbage collection must retain every blob reachable from a retained/pinned release; integrate the selected backend's real GC semantics rather than deleting shared blobs directly.

Digest verification establishes byte integrity against a trusted expected descriptor. It does not authenticate the source, builder, signer, or node. The baseline trusts Kubernetes administrators, controller identities, the embedded registry/proxy identities, and node administrators. A compromised node/runtime is outside the integrity guarantee. State precisely which future signing policy would reduce which trust assumptions.

## Implementation organization and observability

Suggested logical modules are API/schema, reconciliation, release catalog, URI resolution, admission, build definition/format handling, execution strategy, builder adapters, cache policy, registry backend/publication, node-facing endpoint/proxy, and retention. Adapt filenames and binaries to the repository. Avoid putting the entire controller or distribution engine inside `runtime.rs`.

Use `tracing` with structured timestamps, Kubernetes namespace/name/UID, generation, release UID, OCI digest, node UID, operation ID, durations, and error reasons when applicable. Never log secrets or context contents by default. Expose bounded metrics for build/publication/pull-path latency, queue depth, transferred bytes, release/node readiness, retries, and failures. Keep high-cardinality UIDs/digests in logs rather than unbounded metric labels.

## Acceptance criteria

Provide unit tests for URI grammar, exact/range/channel resolution, prerelease behavior, source/definition exclusivity, immutable publication, stale generations, JSON Patch application, and any proxy-readiness affinity conjunction. Mocks supplement actual API/registry/runtime interoperability tests.

Milestone 1 must demonstrate:

1. API-only `kimage-install` on macOS/Linux creates a PVC-backed registry using a cached carrier and mounted static executable, with no preloaded kimage image, external registry, SSH, node trust/configuration changes, socket mounting, or direct injection. Prove bounded helper delivery, binary artifact upload, bootstrap cleanup, interruption/resumption, and the ordinary workload handover.
2. A valid OCI archive is published into the internal registry, preserving root/blob digests. A resolved platform-reference Pod starts through an ordinary kubelet pull with `IfNotPresent` using that digest. Use unique content absent from the node cache and inspect registry requests; a pre-cached startup does not prove endpoint usability.
3. Real node/runtime reachability and TLS/loopback behavior work with unmodified node configuration. Verify the chosen endpoint profile, port availability, namespace policy, auth behavior, and failure diagnostics.
4. Truncated archives, missing blobs, size/digest mismatch, wrong platform, interrupted publication, duplicate attempts, and registry restart cannot publish false readiness or delete shared retained content.
5. Admission dry-run has no side effects; unmanaged containers remain unchanged; missing pull-policy/annotation members produce a valid patch.
6. The operator starts without kimage CRDs, creates them, waits for establishment/discovery, then reconciles an Image. Restarts are idempotent; ownership/RBAC/schema incompatibilities are actionable and non-destructive.
7. API recovery restores registry/proxy serving when all ordinary kimage workloads are stopped and their images are absent, while an allowed carrier/PVC/control plane remain available. Test missing carrier and admission denial as unsupported prerequisites, not automatic fallback to external pulls.
8. Public/external registry access is unnecessary for the exercised installation/build/pull sequence. Explain what egress evidence proves: a Pod NetworkPolicy does not block the host container runtime's pulls. Do not change shared node networking to simulate air-gapping; use an isolated fixture for a fully disconnected proof.

Milestone 2 must execute `source.build.format: Containerfile` with `execution.strategy: Direct` and a selected pinned BuildKit builder. Demonstrate both inline and mounted definitions, ConfigMap text/binary/key/mode mapping, PVC/subPath contexts, build-secret RUN mounts, ordinary RUN bind mounts, frozen snapshots, context-root independence, changed-input cache invalidation, isolated execution, internal OCI publication, and a running resolved Pod. A Secret-sourced definition must not be logged. Verify missing keys/files, path escapes, overlapping mounts, selector conflicts, unsupported formats/strategies/builders/platforms, same-version content conflict, cancellation, and restart recovery.

Exercise Local and Disabled cache modes on the real builder. Cache eviction must not lose retained images; placement/cache policy changes alone must not rebuild an already published release. Common publication/resolution/export interfaces must contain no Containerfile syntax or BuildKit operation types. Future production builders/strategies/shared caches are explicitly unimplemented extensions.

Milestone 3 must demonstrate normal uncached pulls on at least two nodes, rescheduling, new-node endpoint eligibility, proxy and registry restart, runtime-cache eviction followed by normal repull, and defined registry-downtime behavior. Demonstrate a registry/proxy self-upgrade with warmup pulls, interrupted rollout, retained-bundle API recovery, and compatible rollback. Verify operator-owned additive CRD upgrades, restart during reconciliation, old/new operator coexistence without regression, and ordinary uninstall preserving CRDs/PVC/releases. If implementing storage-version migration, verify conversion, data preservation, interruption/resumption, and storedVersions correctness using actual API-server behavior.

Milestone 4 must use a separate actual registry target to prove digest-preserving authenticated external export, tag idempotence/conflicts, interruption/retry, credential rotation, verified remote bytes/digests, and target removal without remote deletion. If the external target is unreachable, internal publication and deployment must still succeed and retry later without rebuilding. Verify the same guarantee with no external exports configured.

Run appropriate Rust formatting, lint, compile, and meaningful tests. Record exact commands, selected versions, results, and limits. Missing cluster access means real-runtime criteria remain unverified; provide a reproducible harness and do not substitute mocks as proof.

### Authorized real-cluster test target

The user has authorized necessary real-cluster tests in namespace `dev-yrashk` on the existing Inferal cluster. Obtain access using the user's shell environment:

`/Users/yrashk/Projects/inferaldata/inferal-workspace/scripts/env.bash`

Source it under Bash. It uses BASH_SOURCE and only runs automatic authentication when sourced interactively; noninteractive sourcing alone may not select/authenticate the kubeconfig. Prefer the existing `$HOME/.kube/inferal-config` if present; otherwise use its `_inferal_k8s_auth` helper as needed. Do not print tokens or full kubeconfig contents. The expected existing context found in the preceding session was `inferal`; verify it before mutation.

A safe initial access check is:

```bash
source /Users/yrashk/Projects/inferaldata/inferal-workspace/scripts/env.bash
if [[ -f "$HOME/.kube/inferal-config" ]]; then
  export KUBECONFIG="$HOME/.kube/inferal-config"
else
  _inferal_k8s_auth
fi
kubectl config current-context
kubectl --request-timeout=10s -n dev-yrashk get pods
```

Inspect namespace policies, existing workloads, runtime/node versions, StorageClasses/PVC availability, and actual permissions before tests. Create uniquely named/labeled test-owned resources in `dev-yrashk`, impose resource/storage limits, and clean up only those resources. Do not delete/recreate the namespace, alter unrelated applications/policies, change node configuration, or install registry-wide mirrors/CA files. Necessary cluster-scoped CRD/RBAC/webhook test artifacts must be uniquely identified, inventoried, and not overwrite definitions/shared policies used by others; use isolated local tests for incompatible schema/downgrade cases when shared-cluster isolation cannot be guaranteed.

The PoCs did create scoped real-cluster resources and successfully ran the bootstrap, archive and build paths. Their temporary workloads/webhooks/RBAC were cleaned up; retained registry PVC, Images/ImageReleases, CRDs and built-in schema coordination metadata remain. See the PoC findings before touching them. Recheck access and inventory before new mutations; use the supported permission route and verified TLS, and preserve unrelated resources.

## Required deliverables

- Architectural decision record, including the tested registry endpoint/bootstrap profile and trust/failure limits.
- Validated Image/ImageRelease schemas, manifests, and operator-owned CRD lifecycle manager.
- Native macOS/Linux kimage-install, complete offline bundle, bounded static bootstrap payloads, API-only registry bootstrap/upload/recovery, and resumable cleanup.
- PVC-backed embedded registry, publication/retention adapter, tested ordinary pull path, and node-local proxy only if required by the chosen endpoint profile.
- Separate format/definition, strategy, builder/placement, and cache contracts; initial Direct strategy and Containerfile/BuildKit adapter.
- Optional independent external registry exporter with credentials/TLS/status/retry handling.
- Compiling code with locked dependencies, meaningful unit/integration tests, and reproducible namespace-scoped real-cluster and isolated disconnected fixtures.
- Operator documentation for bootstrap, build mounts/secrets, releases/channels, registry/PVC backup, endpoint setup, recovery, upgrades/rollback, retention, and uninstall.
- A final report stating completed milestones, passed checks, unverified criteria, and remaining concrete blockers.

Proceed through the architectural review and clean implementation using the PoC evidence below. The basic feasibility gates have passed; reproduce them as regression/acceptance tests in the new implementation rather than opening another open-ended spike. Keep the scope anchored in the working registry-first design.

## Upstream references to verify against selected versions

- Kubernetes images: https://kubernetes.io/docs/concepts/containers/images/
- Kubernetes admission: https://kubernetes.io/docs/reference/access-authn-authz/extensible-admission-controllers/
- Kubernetes image garbage collection: https://kubernetes.io/docs/concepts/architecture/garbage-collection/
- containerd registry configuration: https://github.com/containerd/containerd/blob/main/docs/hosts.md
- containerd localhost resolver source: https://github.com/containerd/containerd/blob/main/core/remotes/docker/config/hosts.go
- Distribution filesystem backend: https://distribution.github.io/distribution/storage-drivers/filesystem/
- OCI image specification: https://github.com/opencontainers/image-spec
- BuildKit: https://github.com/moby/buildkit
- BuildKit Dockerfile frontend: https://github.com/moby/buildkit/blob/master/frontend/dockerfile/docs/reference.md
- Rust semver requirements: https://docs.rs/semver/latest/semver/struct.VersionReq.html
- K3s image archive import: https://docs.k3s.io/add-ons/import-images
- kind image archive loading: https://kind.sigs.k8s.io/docs/user/quick-start/
- Kubernetes volume and mount mapping: https://kubernetes.io/docs/concepts/storage/volumes/
- Kubernetes ConfigMaps: https://kubernetes.io/docs/concepts/configuration/configmap/
- Kubernetes Secrets: https://kubernetes.io/docs/concepts/configuration/secret/
- BuildKit secret consumption: https://docs.docker.com/build/building/secrets/
- OCI Distribution protocol: https://github.com/opencontainers/distribution-spec/blob/main/spec.md
- Kubernetes CRD versioning and stored-object migration: https://kubernetes.io/docs/tasks/extend-kubernetes/custom-resources/custom-resource-definition-versioning/
- Kubernetes RBAC: https://kubernetes.io/docs/reference/access-authn-authz/rbac/
- Kubernetes Pod command overrides: https://kubernetes.io/docs/tasks/inject-data-application/define-command-argument-container/
- Kubernetes Pod exec: https://kubernetes.io/docs/tasks/debug/debug-application/debug-running-pod/
- Kubernetes Pod Security admission: https://kubernetes.io/docs/concepts/security/pod-security-admission/

These URLs identify upstream behavior, not version pins. Select and record concrete compatible versions before implementation.

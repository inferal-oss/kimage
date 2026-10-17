# Frozen OCI base and toolchain inputs

From 0.6.0, a Containerfile can use named, exact versions of already published
Images as bases, toolchains and `COPY --from` sources. Builds still execute on
native amd64 or arm64 workers with networking disabled. The included registry
supplies verified OCI bytes to the session client; BuildKit receives local OCI
contexts and no registry credentials.

## Declare named inputs

First publish `go-toolchain:1.0.0` and `runtime:1.0.0` in `apps`. These are logical
Image versions, independent of the compiler's own version. An administrator can
prepare approved OCI archives and publish them through the existing
[`source.ociArchive` import](https://kimage.dev/docs/#image-api). Provide a complete native index when the
image must support both architectures. This feature does not fetch arbitrary
upstream image tags or start dependency builds.

```yaml
apiVersion: kimage.dev/v1alpha1
kind: Image
metadata:
  name: service
  namespace: apps
spec:
  version: "1.0.0"
  source:
    build:
      format: Containerfile
      execution: {strategy: Direct, builderRef: default}
      imageInputs:
        - name: toolchain
          image: "platform://apps/go-toolchain:1.0.0"
        - name: runtime
          image: "platform://apps/runtime:1.0.0"
      definition:
        inline: |
          FROM toolchain AS build
          WORKDIR /src
          COPY . .
          RUN CGO_ENABLED=0 go build -mod=vendor -trimpath -o /out/service ./cmd/service
          FROM runtime
          COPY --from=build /out/service /service
          ENTRYPOINT ["/service"]
      inputs:
        contextPath: /workspace/context
        producerHandoff: true
        volumes:
          - name: source
            persistentVolumeClaim: {claimName: service-source, readOnly: true}
        volumeMounts:
          - {name: source, mountPath: /workspace/context, readOnly: true}
```

The toolchain must contain the compiler and shell used by the recipe, and all
application dependencies must be provisioned, for example in Go's `vendor/`
directory. An ordinary `FROM scratch` build continues to work without
`imageInputs`.

Aliases are unique lowercase DNS labels beginning with a letter. `scratch`,
`context` and `dockerfile` are reserved. At most eight aliases are allowed.
References must select an exact version in the consuming Image's namespace.
The explicit equality forms `:=1.0.0` and `?version==1.0.0` also
select exact versions; prefer `:1.0.0`. A bare query `?version=1.0.0` is a SemVer
range and is rejected, as are ranges, channels, cross-namespace references and
ordinary registry references.

## What freezes, and when

The first build attempt resolves all declared inputs together. It prefers an
already published complete index over a single-platform release. The controller
stores the dependency Image UID, release UID and full root descriptor in a
per-Image-version immutable snapshot, authenticated with installation-private
key material. Application ConfigMap write access cannot forge that snapshot.
Platform children share it across attempts, operator restarts and later demand.

Every session reads the complete graph through authenticated TLS, verifies sizes,
SHA-256 and layer DiffIDs, then stages local OCI layouts. The native platform
must be present. A later ARM64 request uses the same frozen root as the first
AMD64 build, even if a dependency Image is deleted and recreated with the same
name. It never silently selects the replacement.

If the frozen base contains only AMD64, later ARM64 demand reports
`ImageInputPlatformUnavailable`. Publish the complete base first, then declare
a new consuming Image version. Publishing a new base version does not rebuild
or replace an existing consuming release. Changed output still requires a new
Image version.

## Boundaries and diagnostics

- `ImageInputNotPublished` waits for an already published dependency. kimage does
  not recursively build it, follow a tag, or resolve a dependency cycle.
- `ImageInputVersionConflict` means a version's dependency declaration changed.
  Use a new version. `ImageInputSnapshotAuthenticationFailed` refuses forged or
  damaged snapshot metadata.
- `ImageRelease.spec.provenance.imageInputs` records the exact full root and
  selected native manifest, platform and dependency identities. Private build
  output remains available through `kimage logs` and `kimage builds`.
- `cache.mode: Disabled` still requests a real no-cache solve. Cache eviction
  cannot change retained dependency bytes or published outputs.
- Undeclared `FROM`/`COPY --from` sources and external frontend directives remain
  denied. RUN has no networking; workers keep their egress-denying policy.
- The input graph budget is 1 GiB per session, with a 4 GiB session workspace.
  Existing descriptor, archive, expanded-layer and output limits still apply.
  Installation credentials must be retained across upgrades, as before.

## Upgrade boundary

0.6.0 adds shared CRD revision 6 through a checked, additive update from the exact
shipped revision 5. The registry format and serving protocol remain unchanged.
Upgrade from 0.5.2 with the 0.6.0 CLI, or change the Argo chart version in Git.
Coordinate all installations in a cluster: older controllers cannot start
against revision 6. Returning to 0.5.2 is explicitly refused; kimage never removes
fields, downgrades shared CRDs or discards retained data to make a rollback work.
Same-schema transitions require their own exact compatibility edges.

## Native acceptance

`scripts/e2e-image-inputs.py --run --config PRIVATE_CONFIG.json` targets an
existing installation and uses real native workers. The configuration has the
same namespace/systemNamespace/installation/kubeconfig/context fields as the
[mixed-cluster harness](mixed-cluster-e2e.md), with `probes` containing the
`tool-amd64` and `tool-arm64` fixture executables. Build those with:

```sh
python3 scripts/e2e-image-inputs.py --build-probes .local/image-input-probes
```

Native CI runs the same acceptance on amd64 and arm64, in both installation
scopes and with CLI and Helm. The live mixed test additionally verifies later
platform demand after dependency recreation and missing-platform refusal.
Only test consumer workloads are cleaned up; image releases and registry data
remain retained. Railpack is outside 0.6.0.

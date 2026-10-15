# Optional retained-release exports

Exports are reconciled by a separate `kimage-exporter` Pod and ServiceAccount.
The default installation does not start it. Explicit activation uses native
`kimage-install exporter` with the usual installation/bundle arguments and one or
more `--approved-origin https://registry.example/` values. Only those HTTPS
origins are eligible. With no configured targets, the exporter makes no external
registry requests. The default disconnected build/publication/pull path is
unchanged.

An Image author can add bounded explicit destinations independently of its source:

```yaml
spec:
  exports:
    - name: production
      registry:
        repository: registry.example/team/api-service
        tags: ["1.2.4"]
        credentialsSecretRef:
          name: registry-push
        caSecretRef:
          name: registry-ca
        overwritePolicy: Never
```

The credentials Secret is in the Image's namespace, has type
`kubernetes.io/dockerconfigjson`, and contains an `auths` entry matching the exact
configured authority, including an explicit port when supplied. This initial
transport supports HTTP Basic credentials (`auth` or username/password), plus
explicit `unauthenticated: true` instead of credentials. Credential helpers,
identity-token/Bearer exchange and automatic registry-specific login flows are
unsupported. The CA Secret is optional for publicly trusted certificates and
contains `ca.crt` for a private issuer. HTTPS, hostname checking, fixed-origin
upload locations and refusal of redirects remain mandatory. No node CA or runtime
configuration changes are used.

The exporter selects the verified immutable release matching the live Image UID
and requested exact version. It persists that release UID/root before external
I/O. The source definition, worker, build cache and build controller are not
required to copy it. It downloads and verifies the retained OCI graph, uploads
original blobs/manifests topologically, independently downloads/verifies remote
bytes, and then attaches the explicit tags. Original compressed layers and
manifest/index bytes are preserved. No signature, provenance-referrer or
multi-platform export support is implied beyond the common verifier's supported
single-platform OCI graph.

`Never` is the only supported overwrite policy. An existing tag at the same digest
is idempotent; a different digest is `TagConflict`. Sorted per-destination/tag
Leases serialize this installation's exporter writers across Images and replicas.
Generic Distribution tag PUT has no portable compare-and-swap: another actor or
installation can race the check. Strong global immutability needs registry-side
enforcement. Every resulting tag is read back and checked. Multiple aliases are
not an atomic transaction.

`status.exports` is bounded per target and separate from publication conditions.
It records configuration generation/fingerprint, selected release UID, local and
verified remote digest, canonical remote reference, phase, attempt time, retry
eligibility and a sanitized reason. Credential identity/resourceVersion is used
for retry identity; credential bytes are never hashed into public status. Changing
export targets/tags/credentials does not change the build request digest. The
publisher preserves the latest exporter status when acknowledging its own work.

At most two Image export reconciliations run concurrently, with sequential targets,
a ten-minute target deadline, bounded graph spooling and request deadlines. Retry
backoff grows to 300 seconds; the displayed attempt counter saturates at 32.
Credential/configuration changes reset the retry identity. An API-based monitor
checks cancellation/configuration/credential changes every two seconds, with a
ten-second API deadline; an in-flight HTTP operation may already have reached the
remote server. Exporter restart retries from the persisted retained identity and
normal remote digest checks. A failed target never changes internal publication
readiness or stops local Pods.

Target removal or Image deletion cancels future work. The exporter never deletes
remote content. The baseline retains sealed releases independently of Images and
exporters; removing a target does not garbage-collect the local artifact. Remote
retention and tags changed by other actors remain the target administrator's
responsibility.

The exporter can read Images/releases and named Secrets in the application
namespace, patch only Image status, and coordinate through system-namespace
Leases. It has no node, Pod exec, runtime socket, build, CRD-management or release
mutation permissions. Its mounted internal-store identity is read-only; destination
credentials are obtained only by the exporter and never sent to builders,
admission or node proxies.

The optional API is additive schema revision 4. The operator accepts only exact
shipped revisions 2/3 as upgrade sources, preserves the same served/storage API
version and CRD identities, and refuses unsupported backward schemas. New bundles
record `apiSchemaRevision`; control-plane activation requires the matching native
installer/bundle. Retain earlier independently trusted bundles/installers for
serving recovery, without assuming an older operator can read revision 4.

The transport follows the [OCI Distribution 1.1.1 manifest/tag and blob APIs](https://github.com/opencontainers/distribution-spec/blob/v1.1.1/spec.md).
The separate acceptance registry uses Distribution's supported
[TLS and htpasswd configuration](https://distribution.github.io/distribution/about/configuration/).

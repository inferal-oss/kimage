# Serving binary upgrade and rollback

`upgrade-serving` supports independently trusted bundles containing different
actual Distribution and gateway/proxy executables. The compatibility window is:

| Component | Supported source/target versions |
| --- | --- |
| Distribution filesystem backend | 3.1.1 and 3.1.2, including reverse rollback |
| kimage gateway/proxy | 0.1.0 and 0.2.0, retaining the v1 authenticated endpoint-overlap protocol |
| Operator/admission/input/builder/exporter | Exact same component version and OCI root in both bundles |
| API schema | Matching bundle API revision; this command never changes CRDs |

Both versions must belong to the explicit window, even for same-version
replacement. A different major/patch, other storage driver, configuration override,
data mount, registry command or implicit control-plane change is refused. The
immutable registry ConfigMap must match the shipped filesystem configuration,
with `/data` on the selected PVC and deletion disabled. Compatibility is scoped to
the bytes/configuration and operations tested here, not inferred for arbitrary
Distribution storage drivers or migrations.

The 3.1.1 executable comes from the official
[Distribution release](https://github.com/distribution/distribution/releases/tag/v3.1.1),
archive SHA-256
`6f330a3ba9ea1d23a6ee189f449d792595240585bb2f159123d76ac594f70dd8`.
The release asset's GitHub metadata checksum was checked before extraction and
the actual executable reported 3.1.1. The separately retained 3.1.2 executable
reports 3.1.2. Their executable hashes differ. Prefer
[3.1.2](https://github.com/distribution/distribution/releases/tag/v3.1.2), which
contains security fixes; 3.1.1 is retained for this bounded rollback window.
The first 0.1.0 kimage serving binaries are retained unchanged. The actual 0.2.0
binaries add version reporting; the final proxy also adds bounded local metrics.
They preserve the credential/upstream/namespace-bound v1 overlap challenge.

## Transition protocol

The installer validates both trusted inventories and the compatibility/profile
guards. A renewable installation Lease serializes transitions. Immutable
replacement templates and phase checkpoints bind the target inventory and named
Pod identities. The installer stages/verifies standard OCI graphs through the
registry API and performs ordinary node-targeted warmup pulls while the original
proxies serve. Registry/gateway warm on the primary node; the proxy warms on every
approved node. Warmup must finish before the serving transition.

A temporary read-only registry/gateway mounts the same PVC and serves retained
objects while the single writable registry is replaced. It cannot publish or run
GC. Proxy overlap requires the authenticated v1 endpoint proof; additional-node
approval temporarily permits the exact old/new image references and narrows to
the new image after readiness. UID/resourceVersion fences prevent adoption or
deletion of an unrelated replacement. Every side effect remains API-only.

An interrupted owner can resume the same inventory pair after its Lease expires.
The immutable checkpoint restores a missing writable Pod while the read-only
path remains available. If that serving path is also lost, the independent
retained bundle recreates carrier serving through `recover`, followed by normal
seed/handover/control-plane/builder/join phases. The cached carrier, control plane,
usable node and original PVC remain provider prerequisites.

Rollback reverses the trusted source/target arguments and uses the same protocol.
It never downgrades CRDs or runs a filesystem migration. Ordinary operation returns
to 3.1.2/0.2.0 after rollback verification. A safe resourceVersion conflict may
require repeating a native phase; the retained data/checkpoint is preserved.

## Real acceptance, beyond packaging replacement

`scripts/live-binary-lifecycle.py` composes the reproducible fixture checks:

1. Establish the older real binaries and publish a fresh retained probe release.
2. Check that target serving images are absent from both appropriate CRI caches.
3. Upgrade, check exact target warmups/root requests on both original proxies,
   and kill the installer with the writer absent and read-only serving active.
4. Pull the retained probe uncached through ordinary kubelet while the writer is
   absent; resume the upgrade from the same checkpoint.
5. Execute the running component version commands and publish a new probe under
   Distribution 3.1.2.
6. Roll back all serving binaries and pull that newly written release on the other
   uncached node through Distribution 3.1.1.
7. Uninstall disposable components, evict all eight component references from both
   CRI caches, and recover through the older independently retained bundle.
8. Independently download/verify the complete newer graph after older-bundle
   recovery; roll forward with the packaged unprivileged Linux native installer;
   independently verify the same graph again.

PVC/release/CRD UIDs and specs are preserved throughout. Fixture-only cache
inspection/removal is bounded to exact owned references and is never a product
delivery path. `binary-upgrade-first-results.json` records the first real binary
cycle. `binary-upgrade-results.json` records the final full repetition including
the Linux-native roll-forward. `serving-metrics-final-results.json` records the
later proxy-only HTTP metrics correction through the same native updater;
`observability-results.json` and `export-after-upgrade-results.json` passed after
that correction. All other component artifacts stayed at the validated roots.
The old `serving-upgrade-results.json` remains
historical packaging-only evidence and is not used to establish binary compatibility.

## Exact final fixture commands

These commands ran against the prepared, owned two-node fixture. Credentials stay
in private local files; command arguments contain paths, not credential values.
Use fresh case/output names for a new test. To repeat the uncached-replacement
proof, first perform `live-maintenance-recover.py --evict-system-images` against
the older bundle on both nodes; otherwise previously pulled target images correctly
fail the harness's cache-absence precondition.

```sh
python3 scripts/live-binary-lifecycle.py \
  --kubeconfig .local/qemu-routed/config \
  --options .local/isolated/cold-linux-complete/options.json \
  --initial-bundle dist/complete-rollback-linux-amd64 \
  --previous-bundle dist/complete-rollback-linux-amd64 \
  --bundle dist/complete-linux-amd64 --installer target/release/kimage-install \
  --probe .local/linux-target/x86_64-unknown-linux-musl/release/kimage-probe \
  --admission-credentials .local/isolated/cold-linux-complete/admission-credentials \
  --builder-credentials .local/isolated/cold-linux-complete/builder-credentials \
  --case kimage-binary-a2 --output .local/isolated/binary-upgrade-final \
  --linux-roll-forward

python3 scripts/live-serving-refresh.py \
  --kubeconfig .local/qemu-routed/config \
  --options .local/isolated/cold-linux-complete/options.json \
  --previous-bundle dist/complete-linux-amd64 \
  --bundle dist/complete-final-linux-amd64 --installer target/release/kimage-install \
  --output .local/isolated/serving-metrics-final

python3 scripts/live-observability.py \
  --kubeconfig .local/qemu-routed/config \
  --probe .local/linux-target/x86_64-unknown-linux-musl/release/kimage-probe \
  --admission-ca .local/isolated/cold-linux-complete/admission-credentials/ca.crt \
  --case kimage-direct-metrics-final-a1 --output .local/isolated/observability-final

python3 scripts/live-configure-exporter.py \
  --kubeconfig .local/qemu-routed/config \
  --options .local/isolated/cold-linux-complete/options.json \
  --bundle dist/complete-final-linux-amd64 \
  --installer dist/complete-final-linux-amd64/kimage-install-darwin-arm64 \
  --target .local/isolated/export-target

python3 scripts/live-export-final.py \
  --kubeconfig .local/qemu-routed/config --target .local/isolated/export-target \
  --installer dist/complete-final-linux-amd64/kimage-install-darwin-arm64 \
  --output .local/isolated/export-after-upgrade.json
```

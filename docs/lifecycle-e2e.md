# Installation lifecycle acceptance

Native CI tests the published 0.6.0 predecessor against 0.6.1 on native amd64
and arm64 runners, with CLI/Helm and namespace/cluster scope. Each case uses a
disposable, single-node K3s cluster. The mixed-cluster cases run separately on
real native workers.

The predecessor round trip keeps shared schema revision 6 unchanged. Tests
interrupt and resume upgrades and rollbacks, build fresh Images on both sides,
and confirm unsupported capability requirements prevent returning to an
unprepared 0.6.0 reader before any runtime or journal writes.

A separate, disposable-only fixture installs independently compiled
0.6.2-argocd-test.0 binaries with additive schema revision 7 and populated future
Image and provenance fields. Returning to unchanged 0.6.1 binaries must preserve
the newer CRDs and opaque data, pause unsupported builds, serve published roots
and build unrelated Images. CLI, Helm and Argo CD exercise this foundation.
Never install the synthetic schema fixture on a shared development or production
API. See [compatibility](compatibility.md) for the bounded reader contract.

Acceptance also builds from frozen OCI base/toolchain inputs, checks native
execution, local cache reuse, disabled cache and undeclared-source refusal, then
uninstalls and reinstalls 0.6.1 with retained storage. CLI cleanup stops supervisors
before removal; Helm cases use Helm removal and retire its release history before
installing again.
Fresh-install Helm removal is tested separately. Retained PVC/PV, Image and
release identities and complete OCI readback remain required.

The separate mixed-cluster test is explicitly enabled against an existing cluster.
It needs a [supported runtime](installation.md#prerequisites), administrator access,
Ready Linux amd64 and arm64 nodes, and fresh, dedicated application and system
namespaces. With `nodeChanges: true`, use an amd64 registry node and at least one
additional eligible amd64 node for the worker replacement case.

Build the fixture inputs and fetch the actual published predecessor. Select the
installer platform of the machine running the test; this example uses macOS/arm64:

```sh
rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl
python3 scripts/e2e-mixed-cluster.py --build-probes .local/upgrade-probes
python3 scripts/e2e-image-inputs.py --build-probes .local/image-input-probes
python3 scripts/fetch-predecessor.py --architecture arm64 --system darwin \
  --output .local/upgrade-previous
cargo build --locked --release -p kimage-control --bin kimage-install
```

The predecessor fetch checks GitHub's recorded asset size and SHA-256. Do not
substitute an old checkout compiled with a newer toolchain. Obtain independently
verified target `component-images.json` and `kimage-0.6.1.tgz` from the release, or
from the candidate workflow's `component-catalog-chart` artifact. Candidate images
must pass native acceptance and anonymous GHCR verification before live testing.
See [the release procedure](releasing.md#upgrade-acceptance-for-every-release).

Create a private JSON configuration, replacing paths and names for your cluster:

```json
{
  "kubeconfig": "/absolute/path/kubeconfig",
  "context": "mixed-development",
  "namespace": "kimage-upgrade-test-app",
  "systemNamespace": "kimage-upgrade-test-system",
  "installation": "kimage-upgrade-test",
  "probes": "/absolute/path/kimage/.local/upgrade-probes",
  "output": "/absolute/path/kimage/.local/upgrade-results",
  "installer": "/absolute/path/kimage/target/release/kimage-install",
  "previous": "/absolute/path/kimage/.local/upgrade-previous",
  "catalog": "/absolute/path/kimage/.local/upgrade-target/component-images.json",
  "chart": "/absolute/path/kimage/.local/upgrade-target/kimage-0.6.1.tgz",
  "mode": "cli",
  "scope": "namespace",
  "runtimeProfile": "inferal",
  "port": 17736,
  "registryNode": "amd64-primary",
  "nodeChanges": true
}
```

Use a distinct port, installation identity, output directory and pair of namespaces
for each CLI/Helm run. `mode` accepts `cli` or `helm`; `scope` accepts `namespace`
or `cluster`. Helm must be on PATH for Helm ownership. Cluster-scoped fixtures
exclude every other ordinary namespace existing at installation time.

```sh
python3 scripts/e2e-upgrade.py --run --config .local/upgrade-config.json
```

The test installs the published predecessor, builds and runs a fresh Image,
interrupts the upgrade during application, and resumes with the exact catalog.
It checks retained identities, native image pins, status/doctor, fresh unknown and
explicit placement cases, private failed-attempt output and capability-aware rollback
refusal. Frozen-input tests use fresh consumer Images and, on a mixed cluster,
confirm later ARM64 demand keeps the original base after same-name recreation.
Complete OCI graphs are read back inside the cluster over authenticated TLS, including
after uninstall/reinstall; application layers do not cross the desktop API link.

Node-change acceptance uses a unique fixture label to remove/re-add eligibility,
register an architecture and serving proxies, and move a pinned worker. It restores
original labels and default selection. It never deletes or cordons shared Nodes
or moves the registry's retained storage.

Completion uninstalls disposable system components while retaining the registry
PVC, Images, releases and input fixtures. `results.json` contains sanitized checks,
artifact hashes and runtime image identities. Keep the output directory private:
intermediate files include fixture placement and interrupted-command diagnostics.
If the test runner itself is killed during node changes, use its
`node-labels.private.json` checkpoint to restore only those original label values.
Do not delete retained namespaces or storage as a retry mechanism.

An explicitly excluded, empty existing application namespace can be reused for
serial real-cluster fixtures with `existingEmptyApplicationNamespaceUid` set to
its current UID. This requires namespace scope, a fresh system namespace, and no
Images, ImageReleases or Pods in the application namespace. The fixture preserves
the namespace identity. Removing retained test data between runs is a separate,
explicit cleanup operation after uninstall retention has been verified.

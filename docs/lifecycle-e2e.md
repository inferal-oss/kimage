# Installation lifecycle acceptance

Native CI tests the published 0.4.1 predecessor against the 0.5.0 candidate on
physical amd64 and arm64 runners, with CLI/Helm and namespace/cluster scope. Each
case uses a disposable, single-node K3s cluster. It does not emulate a mixed
cluster with QEMU or nested multi-platform Docker.

Fresh 0.5.0 Helm removal is tested directly. After rollback, the test drains the
published 0.4.1 operator before Helm removal, following the
[documented predecessor shutdown procedure](installation.md). That older
controller can otherwise recreate an ownerless worker during deletion. Cleanup
still requires every disposable system resource to disappear; retained PVC/PV,
Image and release identities and complete OCI readback remain required.

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
python3 scripts/fetch-predecessor.py --architecture arm64 --system darwin \
  --output .local/upgrade-previous
cargo build --locked --release -p kimage-control --bin kimage-install
```

The predecessor fetch checks GitHub's recorded asset size and SHA-256. Do not
substitute an old checkout compiled with a newer toolchain. Obtain independently
verified target `component-images.json` and `kimage-0.5.0.tgz` from the release, or
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
  "chart": "/absolute/path/kimage/.local/upgrade-target/kimage-0.5.0.tgz",
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
interrupts each transition during application, and resumes with the exact catalog.
It checks retained identities, native image pins, status/doctor, fresh unknown and
explicit placement cases, private failed-attempt output and reserved-input denial
after rollback. It also builds a fresh Image after returning to 0.4.1. Complete
OCI graphs are read back inside the cluster over authenticated TLS, including
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

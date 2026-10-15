# kimage-control

Native commands and controllers for [kimage](https://kimage.dev). Declare an Image
and version as a Kubernetes resource. kimage manages production, verified
publication to its cluster-local registry, retained releases, and digest-pinned
workload delivery.

## Install

Requires Rust 1.99.0 or newer. No source checkout is needed:

```sh
cargo install kimage-control --version 0.3.0 --locked
kimage install --help
```

The package installs `kimage`, the compatible `kimage-install` alias, `kimage-operator`, `kimage-admission`,
`kimage-gateway`, `kimage-proxy`, `kimage-input`, and `kimage-exporter`. To install
only the native installer, add `--bin kimage` to the Cargo command.

`cargo install` builds native tools for your machine. The installer fetches the
exact official GitHub release matching its package version, verifies the asset
digests and complete bundle, and caches it before cluster writes. Cluster phases first read the target Node architecture; standalone bundle-fetch needs no kubeconfig. No source
checkout or packaging script is needed. `--release`, `--bundle-cache`, and
`--offline` control selection/reuse; independently pinned local bundles remain
supported with `--bundle` and `--trusted-inventory`. `kimage install` runs bootstrap, seed, handover, control plane, and the default
builder in one resumable command. No separate fetch/check step is needed.
Cluster installation requires TLS credentials and a supported runtime profile. See the
[installation guide](https://kimage.dev/docs/#cluster-setup)
and the [hello tutorial](https://kimage.dev/docs/#quickstart).

Native clients support macOS and Linux on amd64 and arm64. Image placement selects native worker builds, with default on-demand or configurable eager production, and namespace or cluster scope. The initial build
adapter uses Containerfiles, scratch images, and provisioned local inputs.
Read the [compatibility limits](https://kimage.dev/docs/#compatibility)
before installation. The public APIs are alpha.

Selectors also work in Deployments, StatefulSets, DaemonSets, Jobs, CronJobs, and
custom controllers that pass them unchanged into Pod container or init-container
image fields. See [workload compatibility](https://kimage.dev/docs/#workload-compatibility)
for field locations and controller validation requirements.

The crates.io package is called `kimage-control`; the `kimage` crate name belongs
to a separate project. Gateway and proxy retain their serving compatibility
version, currently 0.3.0, independently of the Cargo package version.

Licensed under Apache License 2.0.

# kimage-control

Native commands and controllers for [kimage](https://kimage.dev). Declare an Image
and version as a Kubernetes resource. kimage manages production, verified
publication to its cluster-local registry, retained releases, and digest-pinned
workload delivery.

## Install

Requires Rust 1.99.0 or newer. No source checkout is needed:

```sh
cargo install kimage-control --version 0.2.0 --locked
kimage-install --help
kimage-install bundle-fetch
kimage-install bundle-check --offline
```

The package installs `kimage-install`, `kimage-operator`, `kimage-admission`,
`kimage-gateway`, `kimage-proxy`, `kimage-input`, and `kimage-exporter`. To install
only the native installer, add `--bin kimage-install` to the Cargo command.

`cargo install` builds native tools for your machine. The installer fetches the
exact official GitHub release matching its package version, verifies the asset
digests and complete bundle, and caches it before cluster access. No source
checkout or packaging script is needed. `--release`, `--bundle-cache`, and
`--offline` control selection/reuse; independently pinned local bundles remain
supported with `--bundle` and `--trusted-inventory`. Cluster installation still
requires TLS credentials and a supported runtime profile. See the
[installation guide](https://kimage.dev/docs/#cluster-setup)
and the [hello tutorial](https://kimage.dev/docs/#quickstart).

The tested native clients are macOS/arm64 and Linux/amd64. The initial build
adapter uses Containerfiles, scratch images, and provisioned local inputs.
Read the [compatibility limits](https://kimage.dev/docs/#compatibility)
before installation. The public APIs are alpha.

The crates.io package is called `kimage-control`; the `kimage` crate name belongs
to a separate project. Gateway and proxy retain their serving compatibility
version, currently 0.2.0, independently of the Cargo package version.

Licensed under Apache License 2.0.

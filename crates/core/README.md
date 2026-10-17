# kimage-core

Core library for [kimage](https://kimage.dev), a Kubernetes image production and
delivery system. It provides bounded OCI graph verification, image selectors,
release catalog resolution, admission patch construction, and build contracts.

This is an alpha API. Native Linux/amd64 and Linux/arm64 graphs and verified multi-platform OCI indexes are supported. Admission selects platforms from workload placement and preserves namespace scope.
See the [project documentation](https://kimage.dev/docs/)
for supported formats, runtime profiles, and limits.

For the native installer and operational commands, install `kimage-control`:

```sh
cargo install kimage-control --version 0.6.0 --locked --bin kimage --bin kimage-install
```

Licensed under Apache License 2.0.

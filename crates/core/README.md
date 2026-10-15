# kimage-core

Core library for [kimage](https://kimage.dev), a Kubernetes image production and
delivery system. It provides bounded OCI graph verification, image selectors,
release catalog resolution, admission patch construction, and build contracts.

This is an alpha API. The initially supported image platform is Linux/amd64.
See the [project documentation](https://kimage.dev/docs/)
for supported formats, runtime profiles, and limits.

For the native installer and operational commands, install `kimage-control`:

```sh
cargo install kimage-control --version 0.1.0 --locked
```

Licensed under Apache License 2.0.

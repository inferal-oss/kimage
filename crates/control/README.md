# kimage-control

Native commands and controllers for [kimage](https://kimage.dev). Declare an Image
and version as a Kubernetes resource. kimage manages production, verified
publication to its cluster-local registry, retained releases, and digest-pinned
workload delivery.

## Install

```sh
cargo install kimage-control --version 0.5.0 --locked --bin kimage --bin kimage-install
kimage install --application-namespace apps
```

The installer fetches a small verified release catalog. Kubernetes pulls pinned
native amd64/arm64 component images from public GHCR. Credentials are generated
in process. No component binaries or images are uploaded through the API.
`kimage-install` is a compatible alias. Administrator access, persistent storage
and the supported Pod user-namespace/runtime profile are required.

Alternatively, install the pinned release Helm chart from
`https://kimage.dev/charts`. Both modes support namespace and cluster scope,
preserve application artifacts on removal, and include a local application
registry. See the [installation guide](https://kimage.dev/docs/#cluster-setup)
and [agent essentials](https://kimage.dev/llms.txt).

The crate includes the operator, admission server, helpers and serving tools.
Gateway/proxy retain serving protocol version 0.3.0 independently of Cargo's
package version. The crate named `kimage` is a separate project.

For an existing 0.4.1 installation, update this CLI, then run
`kimage upgrade --to 0.5.0 --check` and `kimage upgrade --to 0.5.0`.
`kimage rollback --to 0.4.1` performs the compatible reverse transition. Both
managers preserve their ownership, registry storage and release metadata.
`kimage status` and `kimage doctor` report installation health. Use
`kimage builds IMAGE -n apps` and `kimage logs IMAGE -n apps --follow` for private,
bounded per-attempt diagnostics. Build output can contain printed secrets.

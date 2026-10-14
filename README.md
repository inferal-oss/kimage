# kimage

Clean implementation baseline following the single-node architecture PoCs.

Start with the [implementation brief](docs/implementation-prompt.md) and
[verified PoC findings](docs/poc-findings.md). Experimental source and full evidence
are preserved on the separate orphan branch `codex/poc-spikes`.

The architecture uses a PVC-backed internal OCI registry, normal kubelet pulls,
Image/ImageRelease declarations and a Rust control plane/native installer. Direct
Containerfile builds use an isolated pinned BuildKit worker. No full implementation
milestone is complete on this branch yet.

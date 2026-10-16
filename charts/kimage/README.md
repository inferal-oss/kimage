# kimage Helm chart

Install the published chart, which contains the release's verified multi-platform
image pins:

```sh
helm repo add kimage https://kimage.dev/charts
helm repo update
helm install kimage kimage/kimage --version 0.4.0 \
  --namespace kimage-system --create-namespace \
  --set applicationNamespace=apps --wait --wait-for-jobs
```

Create application namespace `apps` first. Administrator access and the documented
runtime, Pod user-namespace and storage prerequisites are required. See the
[installation guide](https://kimage.dev/docs/#cluster-setup) for scope, supported
runtime profiles, mirrors, placement, policy, upgrades and retained storage.

`values.yaml` documents the settings. The source chart intentionally has empty
image pins; `scripts/package-chart.py` fills them from the verified release
catalog. Resource JSON is shared with the native CLI through
`scripts/sync-chart-resources.py`. CI compares complete CLI/Helm manifests and
runs actual native installation tests for both managers and scopes.

The chart creates no installer upload Job. Kubernetes pulls system components
from GHCR or the configured mirror. The included PVC-backed registry stores
application releases. Helm removal retains its PVC and the operator-managed
shared CRDs; Image and ImageRelease objects remain retained. Do not delete their
namespaces or storage as ordinary cleanup.

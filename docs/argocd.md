# Install with Argo CD

Starting in 0.5.1, use the published Helm chart with `gitops.enabled: true`.
Argo CD manages a small desired-state ConfigMap and provisioner Deployment, with
its ServiceAccount and installation RBAC. Rendering works offline and is stable:
no live Node objects, certificates or passwords are needed in Git.

The provisioner uses the same native image catalog, runtime checks, namespace
scope, registry storage and guarded lifecycle protocol as the CLI. It discovers
nodes in the cluster, generates credentials once, provisions components and waits
for authenticated readiness. The chart pins the provisioner itself by digest.
Application images continue to use the included retained registry.

## Apply an Application

An administrator must allow this chart's cluster-level installation permissions
in the Argo CD AppProject and destination. The provisioner is an administrator
component, including for namespace-scoped installations; its runtime operator and
admission identities enforce the selected application scope. Prerequisites are the
same [supported runtime, nodes and storage](installation.md#prerequisites) as the
CLI. Argo CD must reach the Helm repository, and Nodes must reach GHCR or a mirror.
No repo-server plugin or shell setup is required.

For all ordinary namespaces, use [the cluster example](../examples/argocd/cluster.yaml):

```yaml
apiVersion: argoproj.io/v1alpha1
kind: Application
metadata:
  name: kimage
  namespace: argocd
  finalizers:
    - resources-finalizer.argocd.argoproj.io
spec:
  project: default
  source:
    repoURL: https://kimage.dev/charts
    chart: kimage
    targetRevision: 0.5.1
    helm:
      releaseName: kimage
      valuesObject:
        gitops:
          enabled: true
        scope: cluster
  destination:
    server: https://kubernetes.default.svc
    namespace: kimage-system
  syncPolicy:
    automated:
      prune: true
      selfHeal: true
    syncOptions:
      - CreateNamespace=true
```

For [namespace scope](../examples/argocd/namespace.yaml), change the values to:

```yaml
gitops:
  enabled: true
scope: namespace
applicationNamespace: apps
```

The provisioner creates the application namespace if missing. The system namespace
must be dedicated and distinct. For K3s, add `runtimeProfile: k3s`; the default
`inferal` profile is the tested RKE2 runtime. Add `excludedNamespaces` for cluster
exclusions, `persistence.storageClass` or `persistence.size` when needed. Use unique
installation identities and ports for additional installations. Overlapping
application scopes are refused.

Argo CD owns the desired state, not a Helm release. `helm list` does not list this
installation. `kimage status` reports manager `argocd`. Runtime objects do not
inherit Argo CD tracking labels or owner references to the provisioner. Worker and
proxy changes therefore do not compete with Argo CD self-healing, and ordinary
syncs do not regenerate credentials.

## Configuration and credentials

Ordinary chart values select scope, storage, placement, mirrors and optional
export. `nodes` and inline `credentials` are refused in GitOps mode. Leave
`buildPolicy` empty and `nodePolicy` null to preserve administrator ConfigMap
edits. Set them explicitly to manage those policies from Git instead. Put worker
pins in `nodePolicy.workerPins`, or use `workerNodes` as its initial shorthand.
The registry's Node name, UID and retained volume never move automatically.

For externally managed credentials, set existing Secret names, all in the system
namespace:

```yaml
gitops:
  enabled: true
  credentialSecrets:
    registry: my-registry-credentials
    admission: my-admission-credentials
    builder: my-worker-credentials
```

Each role is optional. Missing roles are generated in cluster. Registry keys are
`ca.crt`, `tls.crt`, `tls.key`, `reader-password`, `publisher-password`; admission
keys are `ca.crt`, `tls.crt`, `tls.key`, `resolution-key`; builder keys are `ca.crt`,
`server.crt`, `server.key`, `client.crt`, `client.key`. Supply the documented DNS
identities for the installation, valid PEM and independent passwords of at least
32 bytes. CA signing keys are not accepted. Supplied material is validated and
copied into kimage's owned credential Secrets. Changing it later is refused as
credential drift, rather than silently rotating an active identity.

## Health, sync and upgrades

Wait for **Healthy** and **Synced**. A provisioner becomes Ready only after
installation readiness and doctor checks pass. For progress or a safely bounded
failure reason:

```sh
kubectl -n argocd get application kimage
kubectl -n kimage-system logs deployment/kimage-provisioner
kimage status
kimage doctor
```

Repeated sync, self-heal and provisioner restart reuse credentials and retained
storage. Interrupted provisioning retries automatically. Supported release changes
use the same lifecycle Lease, exact-catalog journal, quiescing and retained-identity
verification as CLI upgrades. Keep the desired target unchanged until a pending
operation has completed; selecting another target is refused while it is pending.

Pin `targetRevision` to an exact chart version. Starting with **0.5.1**, changing
this version requests an upgrade or a compatible downgrade. Each later release
must declare and test its exact compatibility edge. No manual catalog override
is needed for the ordinary chart-version path.

During a downgrade, the returning older provisioner automatically starts a
short-lived Job using the installed newer operator's recorded image digest.
That newer binary checks its compatibility contract and performs the transition
under the existing lifecycle Lease and journal. The older provisioner becomes
Ready after the target runtime is healthy. Restarting either provisioner or
helper resumes the recorded operation; keep its desired version unchanged until
completion. Helper Jobs use the provisioner service account, contain no private
material, and are removed automatically after completion or Application removal.
Unsupported transitions are refused before changing runtime components.
If the returning provisioner reports `GitOpsTransitionPending`, inspect the
helper's bounded reason codes:

```sh
kubectl -n kimage-system get jobs -l kimage.dev/gitops-transition=kimage
kubectl -n kimage-system logs job/JOB_NAME
```

Use the returned Job name. Do not delete the lifecycle Lease or journal to force
a transition.

The first Argo CD release is 0.5.1. Version 0.5.0 has no Argo CD ownership support
and cannot be an Argo CD rollback target. CLI/Helm installations retain their
0.5.0 to 0.5.1 transition and reverse path. Forward compatibility is exercised
with a separate `0.5.2-argocd-test.0` conformance build, not a published 0.5.2
release. It tests the 0.5.1 chart returning after a newer runtime is installed.

An optional administrator-trusted `gitops.targetCatalog` can select a supported
runtime while retaining the chart's provisioner. It is a complete verified
component catalog, not a `latest` tag. Unknown releases, incompatible pins and
unsupported transitions are refused. This advanced input does not bypass the
lifecycle contract.

Existing CLI and Helm installations are **not adopted automatically**. Use a
separate non-overlapping installation or retain the existing manager. Do not
change manager annotations or attempt to claim a retained volume from a different
installation.

## Removal and retained data

Deleting or pruning the Application removes the provisioner and its desired-state
resources. It deliberately leaves the runtime installation, registry PVC, Images,
ImageReleases and shared CRDs in place. The system namespace is not a rendered
chart resource. Application deletion is not a data-deletion operation.

Recreating the same Application resumes the same installation and retains its
credentials and data. To remove runtime components as well, first delete the
Application and wait for its provisioner Deployment and transition Jobs/Pods to disappear, then:

```sh
kimage uninstall --namespace kimage-system --installation kimage
```

Uninstall refuses while the provisioner is active, preventing a reconciliation
race. It still preserves the registry PVC, Images, ImageReleases and shared CRDs.
Do not delete the system namespace or retained resources unless you deliberately
intend to discard that installation's data.

For explicit existing-cluster testing and version-transition fixtures, see
[Argo CD acceptance](argocd-e2e.md).

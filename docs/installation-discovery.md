# Find and select installations

All commands use the current Kubernetes context. No discovery command changes
cluster resources, switches contexts, reads credentials or repairs damaged records.

```sh
kimage installations
kimage installations --json
# Namespace-only search works without cluster-wide ConfigMap list permission:
kimage installations --namespace kimage-gitops-system
kimage status
kimage doctor
```

Discovery lists installation identity, system namespace, release, manager and scope.
It follows paginated API results and keeps invalid installation records visible.
These records may require a matching older CLI or administrator repair. Uninstalled
volumes without an installation record are not reported as live installations.

## Automatic fallback

`status`, `doctor`, `upgrade`, `rollback` and `uninstall` first look for `kimage`
in `kimage-system`. If it exists, they use it. If its installation record is
missing, they discover other installations. They do not treat permission errors,
API failures or a damaged existing record as evidence that the installation is
absent.

- Status and doctor inspect **every** match and identify each result. A failed
  inspection does not stop the remaining checks. Doctor exits nonzero if any
  installation needs attention; status exits nonzero if any inspection fails.
- Upgrade, rollback and uninstall require exactly one match. Several matches
  produce a list of explicit selection flags before any mutation or catalog fetch.
- `--namespace SYSTEM` constrains discovery to that system namespace.
  `--installation NAME` constrains discovery to that identity. Both flags select
  an exact target and never fall back to another installation.
- `--no-discovery` requires the conventional or explicitly selected target and
  reports a missing installation with a short explanation and next steps.

```sh
kimage status --namespace kimage-gitops-system --installation kimage-gitops
kimage doctor --no-discovery
kimage upgrade --to 0.5.2 --check
```

`install` creates or resumes the requested identity; it never redirects to a
discovered installation. Logs/builds select an Image in an application namespace,
not a default installation. Their namespace selection is unchanged. Legacy
bootstrap operations already require explicit identities.

## Permissions and JSON

Cluster-wide discovery needs `list` on ConfigMaps across namespaces. Namespace-only
discovery needs ConfigMap `list` in that namespace. Exact selection needs ConfigMap
`get`, then the ordinary permissions for the requested operation. Discovery does
not request Secrets or change RBAC. If listing is forbidden, use an accessible
system namespace or both exact selection flags; permissions are not silently
bypassed.

A single status/doctor result keeps the existing JSON object. Multiple results
use `{"schema":1,"healthy":false,"installations":[...]}`; `healthy` reflects all
results. A failed inspection has identity, `healthy: false` and an `error` string.
`installations --json` always returns `{"schema":1,"installations":[...]}`;
no matches is an empty list. Diagnostic commands with no matching installation
exit nonzero with context and installation guidance. JSON stdout contains no
progress or selection messages.

## Argo CD removal

An Argo-managed installation requires explicit acknowledgment before CLI uninstall:
terminal users must type `yes`, and automation must supply `--allow-argocd`.
The warning identifies the installation and explains GitOps ownership.

Remove its Application from Git first, then wait for the provisioner Deployment,
provisioner Pods and transition Jobs/Pods to disappear. Only then run:

```sh
kimage uninstall --namespace kimage-gitops-system --installation kimage-gitops --allow-argocd
```

The flag acknowledges ownership; it does not bypass active-provisioner checks,
manager checks, leases, pending operations or retained-data protections. It does
not delete or edit the Argo Application. Runtime uninstall preserves registry data,
Images, releases and shared CRDs. See [Argo CD removal](argocd.md#removal-and-retained-data).

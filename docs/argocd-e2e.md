# Argo CD acceptance

This opt-in test uses an existing Argo CD controller and isolated kimage
Applications. It is separate from ordinary installation instructions in the
[Argo CD guide](argocd.md). Native CI also runs it against disposable Argo CD
controllers on real amd64 and arm64 runners.

## Existing mixed cluster

Use administrator kubeconfig for a supported cluster with Ready Linux amd64
and arm64 nodes, the documented runtime prerequisites, persistent storage and
Argo CD already installed in `argocd`. The default AppProject must permit the
public source, fixture destination and installation RBAC. The harness does not
install or restart shared Argo CD components or change unrelated Applications.

Prepare the native probe executables described in the
[mixed-cluster acceptance guide](mixed-cluster-e2e.md). Use a verified candidate
component catalog and matching installer. Keep configuration outside Git:

```json
{
  "kubeconfig": "/private/path/to/single-context-kubeconfig",
  "context": "approved-context",
  "namespace": "kimage-argo-test-app",
  "systemNamespace": "kimage-argo-test-system",
  "installation": "kimageargotest",
  "probes": "/private/path/to/native-probes",
  "output": "/private/path/to/new-acceptance-output",
  "catalog": "/private/path/to/component-images.json",
  "installer": "/private/path/to/kimage",
  "scope": "cluster",
  "port": 17751,
  "registryNode": "amd64-registry-node",
  "runtimeProfile": "inferal",
  "sourceRevision": "EXACT_TESTED_GIT_COMMIT",
  "nextCatalog": "/private/path/to/conformance-component-images.json",
  "nodeChanges": true,
  "timeoutSeconds": 900
}
```

Use fresh fixture namespaces and Application identity, a new output directory,
and a port unused by other installations. Fixture names must start with
`kimage-`; the installation name must start with `kimage`. Registry placement
must identify an eligible amd64 node for the mixed node-change checks.
`scope: namespace` tests the other runtime scope. Fixture storage uses
`local-path`. Existing ordinary namespaces are excluded from cluster scope.

```sh
python3 scripts/e2e-argocd.py --run --config /private/path/to/config.json
```

Without `--run`, the harness refuses before reading configuration or contacting
a cluster. Node-change acceptance uses only a fixture-specific label, restores
its prior values, and never deletes or cordons Nodes. It temporarily removes
eligibility from the arm64 node and an ordinary amd64 node, checks worker fencing
and proxy revocation, then restores both.

## Version transitions from 0.5.1

`nextCatalog` enables a real chart-version round trip through the separate
`0.5.2-argocd-test.0` conformance build. Native workflows build its actual
versioned binaries, frozen 0.5.1 templates and exact compatibility contract in
an isolated workspace. Candidate publication uses separate conformance image
tags. This is not a Cargo or GitHub release of 0.5.2.

The Application selects the conformance chart at
`tests/fixtures/argocd-next-chart`, then returns to `charts/kimage` at the same
exact Git commit. Their chart/app versions differ. The test interrupts the
provisioner during upgrade; during downgrade it checks the installed newer
operator digest, interrupts its helper with a persisted incomplete journal,
and restarts the returning older provisioner. Fresh application Pods resolve
and execute retained native releases after both transitions. Storage,
credentials, installation and release UIDs must remain unchanged.

For acceptance of the final published chart repository, omit `nextCatalog`
and replace `sourceRevision` with:

```json
{"chartRepository": "https://kimage.dev/charts"}
```

That path selects the exact public 0.5.1 chart. It repeats installation,
reconciliation, native demand/build/pull, refusal and retention checks. It uses
the published chart's own image pins without image overrides, then compares
actual native component identities against the independently verified catalog.

## Results and cleanup

The sanitized `results.json` records completed checks, catalog/harness hashes,
version transitions and retention results. The nested build receipt records
fresh Image UIDs, native roots and consumer placement. Credential bytes and
private build output are not included.

The harness waits for provisioner Deployments, their Pods and transition Jobs/Pods
to disappear before explicit runtime removal. It records the retained claim and
release UIDs before that command. A failed local uninstall saves
`uninstall.private.log` with owner-only permissions; keep it private. CI records
only the exit code and allowlisted reason codes, without copying command output.

Successful acceptance deletes its Application and disposable runtime through
the supported removal path. It removes only its consumer Deployments/Pods and
preserves fixture namespaces, registry PVC, Images and releases. Failure leaves
bounded diagnostic metadata and may leave the isolated Application/runtime for
inspection. Use the same fixture identity with the documented Application and
runtime removal procedure; never remove shared Nodes, unrelated Applications,
retained data or shared CRDs to clean up a test.

To repeat the full test on its already-uninstalled retained fixture, use a new
output directory and explicitly supply the observed namespace and registry claim
UIDs. This avoids deleting data to recreate a supposedly fresh fixture:

```json
{
  "retainedFixture": {
    "namespaceUids": {
      "kimage-argo-test-app": "EXACT_APPLICATION_NAMESPACE_UID",
      "kimage-argo-test-system": "EXACT_SYSTEM_NAMESPACE_UID"
    },
    "pvcUid": "EXACT_INSTALLATION_DATA_CLAIM_UID"
  }
}
```

This option refuses replaced namespaces/claims, an existing installation record
or active owned runtime resources. It preserves the previous receipt and retained
data; every image placement case still creates a fresh Image UID. Ordinary runs
continue to require fresh namespaces.

The 0.5.1 [native acceptance receipt](native-0.5.1-results.json) and
[real mixed-cluster receipt](mixed-0.5.1-results.json) record the completed
checks and their exact catalogs. Historical attempts remain distinct from the
subsequent complete repeat.

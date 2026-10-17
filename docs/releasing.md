# Releasing kimage

Public Cargo packages are `kimage-core` and `kimage-control`. The crate named
`kimage` is unrelated. Starting with 0.4.0, GitHub Actions publishes native
component images to GHCR and a Helm chart to the website's chart repository.
The unpublished `kimage-bootstrap` helper remains for legacy recovery tooling.

## Prepare and validate

Update the workspace version, exact core dependency, Cargo.lock, both crate
READMEs, README, dated Keep a Changelog section/compare links, chart version and
appVersion, handbook, website commands, clipboard command and llms.txt together.
Gateway/proxy retain their separate serving compatibility version, currently
0.3.0. A Cargo version bump does not change that contract.

```sh
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
python3 scripts/check-changelog.py
python3 scripts/test-release.py
python3 scripts/test-component-images.py
python3 scripts/test-mixed-e2e.py
python3 scripts/test-install-e2e.py
python3 scripts/test-upgrade-e2e.py
python3 scripts/test-argocd-e2e.py
python3 scripts/test-argocd-next.py
python3 scripts/test-helm-diagnostics.py
python3 scripts/check-docs.py
python3 scripts/check-site.py
python3 scripts/check-chart.py --helm helm
python3 scripts/test-gitops-chart.py --helm helm
cargo package --locked --workspace --exclude kimage-bootstrap
```

Canonical resource JSON lives in `crates/control/src/pull_install/templates`.
Run `scripts/sync-chart-resources.py` after changing it. CI compares complete Helm
and native CLI resources for both scopes and architectures, including optional
exporter RBAC, credentials, image pins and retained storage.

Review all eight native Kubernetes acceptance cases: CLI/Helm, namespace/cluster,
amd64/arm64. Each new disposable K3s fixture pulls components from a TLS registry
with complete native indexes; kimage images are never imported into containerd.
The tests cover authenticated workers, real builds and application pulls, demand,
eager policy, controller compatibility, scope/RBAC, idempotence and retained data
on uninstall. Mixed-node placement acceptance remains an explicitly enabled
real-cluster test, separate from the native CI matrix.

## Review a preview

Commit reviewed source to main, then:

```sh
gh workflow run publish.yml --ref main -f dry_run=true
```

The workflow validates source/docs and runs native acceptance. Linux builds use
native musl toolchains; macOS installers are also built natively. Pinned upstream
Distribution 3.1.2 and BuildKit 0.33.0 inputs are size/SHA-256 checked before named
executables are extracted. Component packaging checks static ELF architecture,
compressed-layer DiffIDs, complete OCI graphs and independent native manifests.
The eight deterministic multi-platform indexes contain both amd64 and arm64.
No QEMU build emulation is used.

Review `release-assets`:

- Four `kimage-v0.5.2-<linux|darwin>-<amd64|arm64>` installer binaries.
- `component-images.json`, the bounded catalog with all eight immutable index pins.
- `release-compatibility.json`, matching the compiled transition edges and persisted formats.
- `kimage-0.5.2.tgz`, containing the same canonical resources and release pins.
- `LICENSE`, `THIRD-PARTY-NOTICES.txt`, `SHA256SUMS` and release notes.

`component-layouts` is an internal build artifact used by publication and CI,
not an administrator installation input. Source chart digest placeholders are
filled only after both native graph sets pass verification.

## Tag and publish

```sh
git tag -a v0.5.2 -m 'kimage 0.5.2'
git push origin v0.5.2
gh workflow run publish.yml --ref v0.5.2 -f dry_run=true
# After reviewing the exact tag preview:
gh workflow run publish.yml --ref v0.5.2 -f dry_run=false
```

The tag must match the workspace version and point at a main commit. Publication
uses the standard OCI registry protocol with all architectures and preserved
digests. Images have source/license/version labels and live at
`ghcr.io/inferal-oss/kimage/<role>:v0.5.2`; installation uses their index digests.
Existing different release tags are never overwritten.

**First publication:** new GHCR packages can default to private. Set all eight
component packages to public in the organization's package settings. The job
waits for and verifies anonymous access to each index, both child manifests and
configs before publishing the Cargo packages and finalizing the GitHub release.
Do not advertise an installable release while its images require private access.
The workflow uses a job-limited `GITHUB_TOKEN` with `packages: write`; no long-lived
publishing token is stored.

Both crates trust only repository `inferal-oss/kimage`, workflow `publish.yml`,
environment `crates-io`, with tags `v*`. The Rust OIDC action exchanges that
identity for a temporary crates.io token. Core publishes before control. Exact
existing crate checksums are verified on retries; conflicting bytes stop the run.
Assets are staged and SHA-256 checked in a draft release, then made public only
after image access and Cargo publication succeed. Published tags/assets remain
immutable. Never move a published tag or bypass digest conflicts during retry.

## Website and Helm repository

Release completion dispatches Pages. The website gate waits for both Cargo
packages and all required public assets. Pages fetches every public release chart
using GitHub's recorded asset SHA-256, validates its archive/version and writes
`site/charts/index.yaml` plus the versioned `.tgz` files. Earlier charts remain
available. The apex repository is `https://kimage.dev/charts`.

Before publishing website changes, preview the installation page and capture a
screenshot. After deployment verify live handbook/llms.txt versions, anonymous
GHCR pulls, `helm repo add/update` and `helm show values` for the released chart,
and the released native installer's version/help/catalog fetch path. Check all
workflow conclusions and a clean pushed main before reporting completion.

Older 0.2.x/0.3.x tags keep their original bundle publication workflows. Retain
those installers and independently trusted bundles for legacy recovery; 0.4.0
does not perform automatic adoption, storage migration or schema downgrade.

## Upgrade acceptance for every release

Keep the previous published release's canonical resources frozen under
`crates/control/src/pull_install/templates/vVERSION`. Do not regenerate a rollback
from new resource templates. Update `release-compatibility.json` with the exact
allowed edge, data/API/installation/build formats and any downgrade constraints.
The component catalog and protocol versions must agree. Published predecessor
assets come from GitHub with recorded size/SHA-256 verification, not a recompiled
old checkout. Keep historical evidence unchanged.

Native CI runs CLI/Helm, both scopes and both architectures through the published
predecessor, interruption/resume and reverse transition. It builds fresh Images,
consumes outputs produced on each side, checks private failed-attempt diagnostics
and predecessor input protection, and verifies every fixture OCI graph inside the
cluster after rollback and after uninstall/reinstall. Secret material never enters
public receipts. The real mixed-node test is explicitly enabled separately with
`scripts/e2e-upgrade.py --run --config PRIVATE_CONFIG.json`. See the
[lifecycle acceptance guide](lifecycle-e2e.md) for configuration and retained data.

Before the final tag, run the `Release candidate images` workflow on main. It first
passes native acceptance, then publishes immutable `candidate-COMMIT` GHCR tags.
An optional `source_run` reuses a successful Native Kubernetes E2E run after
checking a main-branch push/manual run with all eight matrix cases, source ancestry and identical product, chart,
packaging and acceptance sources. Documentation or candidate-workflow changes
can follow that run; changes to those tested sources require new acceptance.
Failed, partial or pull-request runs cannot publish candidate images. Main-branch
and release acceptance include repeated private-failure diagnostics on each
native architecture.
Installation still selects digests from the candidate catalog. Candidate acceptance
repeats failed-attempt output checks with fresh Images on each native platform to
catch batching and recovery races. This permits real
mixed-cluster testing without claiming or overwriting final release tags. Use
isolated namespaces/ports and reversible fixture labels for node membership;
never delete or cordon shared Nodes. Restore selection before testing an older
operator that cannot enforce it. Review actual candidate catalogs and retain
public, sanitized acceptance receipts. Final release pins must match the tested
candidate when code is unchanged.

For a failure whose Helm stderr may contain rendered Secrets, the candidate
workflow accepts an optional `diagnostic_certificate`: a public X.509 encryption
certificate. Keep its matching private key outside GitHub. The test-only wrapper
captures failed Helm output directly into CMS/AES-256 ciphertext; artifacts contain
only `.p7m` files. Decrypt locally with the recipient key and review privately.
Ordinary CI and publication do not enable this capture. Never upload plaintext
Helm failure output or the recipient private key as acceptance evidence.

The manual `Focused native lifecycle diagnostics` workflow can reuse a prior native
component build run when `Cargo.toml`, `Cargo.lock` and `crates/` are unchanged.
Supply its exact run ID and source commit. A public recipient certificate is
optional; omit it for metadata-only diagnostics without output capture.
It checks source identity and ancestry, recreates disposable native amd64 and
arm64 fixtures for both scopes and installation managers, and runs the same fresh-install and lifecycle sequence without
rebuilding components. It has no publication permissions.
It repeats private failed-attempt checks with fresh Images to expose timing
failures while preserving the normal acceptance assertions.

Native failure summaries report only allowlisted phase/public status codes and bounded output
metadata. They do not upload private attempt Secrets or raw build output.

## Argo CD acceptance

From 0.5.1, native E2E also provisions an isolated Argo CD v3.5.3 controller
and installs the exact source chart through a real Application. The upstream
manifest is size/SHA-256 pinned. The same opt-in harness,
`scripts/e2e-argocd.py --run --config PRIVATE_CONFIG.json`, targets the existing
Argo CD in the real mixed cluster. It never restarts shared Argo controllers,
modifies unrelated Applications, deletes Nodes or deletes retained data.
Check both scopes, initial interruption/recovery, repeated sync, self-heal,
isolated controller restart, administrator policy, native builds/pulls,
unsupported transition refusal and retained identity after Application removal
and reinstall. Mixed acceptance additionally changes only reversible fixture
Node labels to test lost/new native architectures and proxies.
Before final publication, preserve a sanitized receipt for the tested candidate.
After publication, run the public chart through its Helm repository too.

## Argo CD transition protocol

From 0.5.1 onward, retain `provision --configuration PATH --once` and the
`KIMAGE_GITOPS_CONFIGURATION`/`KIMAGE_GITOPS_INSTALLATION_UID` handoff checks.
The returning older chart delegates guarded downgrades to the installed newer
operator by its recorded digest. A later compatible release must retain its
predecessor templates, declare the exact transition edge, and preserve the
provisioner protocol recorded in the compatibility manifest. Never loosen the
compatibility check to accept arbitrary future releases.

Native acceptance builds a separate `0.5.3-argocd-test.0` workspace and chart to
exercise a real version round trip from 0.5.2, including interrupted helper and
provisioner recovery. Conformance image tags are isolated from release tags;
this fixture is never published to Cargo or as a GitHub release. Real mixed
acceptance repeats this against the existing Argo CD controller.

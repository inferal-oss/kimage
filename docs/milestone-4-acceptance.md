# Milestone 4 acceptance

Passed on the two-node pinned fixture, including final-binary initial-status and
retained-export checks. All transfers use generated probe/message fixtures and a
separate private ClusterIP registry in the same disconnected owned cluster, with
an independent PVC/issuer/authentication. No shared Inferal or outside registry is
used. The payload/destination checks are recorded in `export-fixture-boundary.json`.

| Gate | Evidence |
| --- | --- |
| Authenticated digest-preserving transfer | `export-results.json`: the exact retained release UID/root exported with the source ConfigMap, worker and build controller absent. A separate native client downloaded and verified the entire remote graph and tag. |
| Explicit aliases and Never policy | Same-root alias idempotence passed. A different internally published root reported TagConflict, stayed internally Ready, and left the original remote alias unchanged. |
| Interrupted exporter and retry | The exporter was stopped at its persisted Transferring checkpoint, restarted, waited out its previous tag Lease, and completed against the same retained release UID/root. This proves restart before acknowledgment; it does not claim byte-offset upload-session resumption. |
| Destination unavailable | A fresh Image built/published internally and a fresh normal node-2 Pod pulled it while the separate registry was stopped. Export retried against the same release after destination recovery, without rebuilding. |
| Credential rotation | The actual target's Basic-auth password changed first; stale client credentials failed independently. Updating the same-namespace dockerconfigjson Secret allowed completion for the same retained UID/root. |
| Removal and retention | Removing the target cleared its independent status. Remote graph/tag readback still succeeded. The exporter never issued remote deletion. |
| Credential separation | Old/new credential bytes were absent from Image status and exporter logs. The build context contained only a probe executable and generated fixture message. Actual RBAC checks in `export-rbac-results.json` deny node/Pod-exec/CRD/release mutation access. |
| No exports configured | The first fresh publication completed before adding targets; its source/worker/controller could then be removed before exporting the retained bytes. The default path does not start the optional exporter or require external access. |
| Additive API/bundle | `export-maintenance-results.json` records native API-only activation and operator-owned revision 3 → 4 schema expansion, preserving PVC/release/CRD identities. No custom API storage version changed. |
| Final coherent bundle | `export-final-maintenance-results.json` records deployment of the final eight-component bundle with all retained identities/specs preserved. A safe resourceVersion conflict during handover resumed from the same native checkpoint. |
| Initial independent status | `export-final-results.json` records PendingRelease/observedGeneration 0 before any build controller acknowledgment, then a retained export on the final binary with the controller absent. Independent remote graph/tag verification and controller restoration passed. |
| Post-binary-upgrade regression | `export-after-upgrade-results.json` repeats independent PendingRelease and retained export with the publisher absent after the real upgrade/rollback/recovery cycles and final proxy correction. Exact remote graph/tag readback, target removal and publisher restoration passed. |

The final bundle includes eight component OCI archives, static bootstrap tooling,
pinned BuildKit 0.33.0 and inventoried native macOS/arm64 and Linux/amd64 installers.
API schema revision is explicit in its inventory. The current final bundle and
47-test macOS/Linux suites are recorded in `final-bundle-results.json`; the older
export-era bundle receipt is retained in `export-bundle-results.json`. Formatting,
strict macOS Clippy and Python syntax checks pass.

See `export-design.md` for exact behavior and limits: HTTPS Basic or explicit
anonymous authentication, approved origins, private CA support, bounded targets,
concurrency/deadlines/backoff/cancellation, no generic tag CAS guarantee, no remote
cleanup, no token-helper/Bearer flow, and no signing/referrer or multi-platform
claim. This is an optional destination; the internal retained registry remains
authoritative and its single-PVC failure limits remain unchanged.

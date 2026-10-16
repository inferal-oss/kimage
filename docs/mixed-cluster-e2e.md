# Opt-in mixed-cluster E2E

This test uses real Linux/amd64 and Linux/arm64 nodes. It is explicitly enabled,
separate from the homogeneous native Kubernetes CI matrix. Public CI checks the
harness contract and skips the live Rust test by default; it does not emulate a
mixed cluster or require access to a private cluster.

## Prerequisites

Use a dedicated application namespace managed by an existing kimage installation.
Both native workers must be Ready on matching physical node architectures, with
node-local image serving available on nodes eligible for the consumers. The
installation must already use the default `OnDemand` build policy. The test
checks the policy and never changes it or installs components.

The kubeconfig identity needs to read Nodes and the installation's configuration,
workers, and ImageDemands, and to create/read Images, ConfigMaps, Deployments,
Pods, and ImageReleases in the test application namespace. It also needs Pod logs
and permission to delete its own Deployments. Use administrator-approved test
access. Supply an explicit kubeconfig and context; the runner never changes the
current context.

Install Python 3, kubectl, and Rust with both musl targets. Build the tiny static
probes locally, without Docker or cross-architecture emulation:

```sh
rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl
python3 scripts/e2e-mixed-cluster.py --build-probes /tmp/kimage-mixed-probes
```

The runner uses Rust's self-contained linker and verifies the probes' ELF
architectures and static linkage before creating inputs. The unknown-placement
case uses required Pod anti-affinity on `kubernetes.io/arch` to spread two Pods
across architectures while leaving each Pod's node placement unconstrained.
Cluster admission policy must permit that topology key.

## Run explicitly

Create a JSON configuration outside the repository's tracked files. Paths should
be absolute. The output directory must not already exist; use a new directory
for each run. Keep kubeconfig credentials in the kubeconfig, not this JSON.

```json
{
  "kubeconfig": "/absolute/path/to/kubeconfig",
  "context": "mixed-cluster",
  "namespace": "kimage-mixed-test",
  "systemNamespace": "kimage-system",
  "installation": "dev",
  "probes": "/tmp/kimage-mixed-probes",
  "output": "/tmp/kimage-mixed-results-1",
  "timeoutSeconds": 600,
  "settleSeconds": 10
}
```

Run the specially tagged Rust integration test:

```sh
KIMAGE_MIXED_E2E_CONFIG=/absolute/path/to/config.json \
  cargo test --locked -p kimage-control --test mixed_cluster -- --ignored --nocapture
```

Alternatively, enable the same runner directly:

```sh
python3 scripts/e2e-mixed-cluster.py --run --config /absolute/path/to/config.json
```

Without `--run`, the Python script skips without accessing Kubernetes. Standard
`cargo test --workspace` also skips the live test. Harness-only checks are safe to
run without cluster access:

```sh
python3 scripts/test-mixed-e2e.py
```

## What it proves

Every case creates a fresh Image UID, immutable source ConfigMap, and consumer
Deployment. Images omit both `spec.platform` and `spec.platforms`, so placement
must drive demand. Each Image first waits for demand with no native children,
releases, or demand record. The test then creates its consumer and checks:

| Consumer placement | Expected native builds | Selected output | Runtime check |
| --- | --- | --- | --- |
| Unknown | amd64 and arm64, exactly | Verified OCI index | Both architectures pull the same index root and execute their native program |
| Explicit amd64 | amd64 only | Verified native manifest | Program runs on a real amd64 node |
| Explicit arm64 | arm64 only | Verified native manifest | Program runs on a real arm64 node |

A unique input marker makes each native `RUN` execute for the fresh case. The
probe records its build architecture inside the image, then checks it at runtime.
Assertions inspect current demand generations, exact child and release sets,
parent/child UIDs, native execution provenance, verification, and the actual
consumer digest. Stored Deployment templates must retain their `platform://`
selector. A final settling check rejects late unwanted builds and confirms that
administrator build policy was unchanged.

`results.json` records completion, the three Image UIDs, native child UIDs,
selected roots, consumer nodes, and cleanup outcome. A successful process exit
and `complete: true` with `cleanupComplete: true` are required for a passing run.

## Retention and cleanup

The runner deletes only the consumer Deployments it created, using UID
preconditions, and waits for their Pods to disappear. It deliberately retains
its Images, input ConfigMaps, demand records, and published artifacts for
inspection. A new run uses new names and UIDs. Use a dedicated namespace and the
installation's normal retention/decommission procedure to remove the fixture
when finished. Deleting a workload or input alone does not reclaim retained
registry bytes. Do not point the test at a production application namespace.

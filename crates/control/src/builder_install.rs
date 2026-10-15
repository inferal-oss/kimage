//! Administrator-owned, offline OCI worker. No node runtime socket or host volume.
use crate::{bundle::Bundle, install::Installer};
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use k8s_openapi::api::core::v1::Pod;
use kube::Api;
use serde_json::json;
use std::{collections::BTreeMap, path::Path, time::Duration};

// Versioned configuration is kept private to this engine adapter. Insecure
// entitlements, delegated frontends and build history are disabled.
pub(crate) const CONFIG: &str = r#"
root = "/cache"
insecure-entitlements = []
[log]
level = "panic"
[grpc]
address = ["unix:///run/buildkit/buildkitd.sock", "tcp://0.0.0.0:1234"]
[grpc.tls]
cert = "/credentials/server.crt"
key = "/credentials/server.key"
ca = "/credentials/ca.crt"
[history]
maxEntries = 0
[cdi]
disabled = true
[worker.oci]
enabled = true
platforms = ["linux/amd64"]
snapshotter = "native"
rootless = true
noProcessSandbox = false
binary = "/buildkit-runc"
networkMode = "host"
max-parallelism = 1
gc = true
reservedSpace = "256MiB"
maxUsedSpace = "2GiB"
minFreeSpace = "1GiB"
[[worker.oci.gcpolicy]]
all = true
reservedSpace = "256MiB"
maxUsedSpace = "2GiB"
keepDuration = "24h"
[worker.containerd]
enabled = false
[frontend."dockerfile.v0"]
enabled = true
[frontend."gateway.v0"]
enabled = false
"#;
// BuildKit 0.33.0 generates its executor spec with containerd 2.3.4's
// defaultUnixCaps (pkg/oci/spec.go). The daemon's bounding set must contain
// those capabilities, plus SYS_ADMIN for mount setup and NET_ADMIN for runc's
// private loopback setup. All remain Pod-userns
// scoped; the nested executor's default set does not include SYS_ADMIN.
const WORKER_CAPABILITIES: &[&str] = &[
    "CHOWN",
    "DAC_OVERRIDE",
    "FSETID",
    "FOWNER",
    "MKNOD",
    "NET_RAW",
    "SETGID",
    "SETUID",
    "SETFCAP",
    "SETPCAP",
    "NET_BIND_SERVICE",
    "SYS_CHROOT",
    "KILL",
    "AUDIT_WRITE",
    "SYS_ADMIN",
    "NET_ADMIN",
];

impl Installer {
    pub async fn builder(&self, bundle: &Bundle, credentials: &Path) -> Result<()> {
        ensure!(
            self.options.namespace != "default" && !self.options.namespace.starts_with("kube-"),
            "DedicatedSystemNamespaceRequired"
        );
        let component = bundle
            .inventory
            .components
            .get("builder")
            .context("DefaultBuilderMissingFromBundle")?;
        ensure!(
            component.version == "0.33.0",
            "UnsupportedDefaultBuilderVersion"
        );
        let mut material = BTreeMap::new();
        for name in [
            "ca.crt",
            "server.crt",
            "server.key",
            "client.crt",
            "client.key",
        ] {
            let path = credentials.join(name);
            ensure!(
                std::fs::symlink_metadata(&path)?.is_file(),
                "InvalidBuilderCredentialFile"
            );
            let bytes = std::fs::read(path)?;
            ensure!(
                !bytes.is_empty() && bytes.len() <= 32768,
                "InvalidBuilderCredentialSize"
            );
            material.insert(name, bytes);
        }
        reqwest::Certificate::from_pem(&material["ca.crt"])?;
        for prefix in ["client", "server"] {
            let mut pem = material[format!("{prefix}.crt").as_str()].clone();
            pem.extend_from_slice(&material[format!("{prefix}.key").as_str()]);
            reqwest::Identity::from_pem(&pem).context("InvalidBuilderCertificateIdentity")?;
        }
        self.preflight().await?;
        // The worker has its own journal: adding its offline component does not
        // rewrite the already completed registry/proxy handover checkpoint.
        self.state_named(bundle, "DefaultBuilderPending", "builder-state")
            .await?;
        let image = format!(
            "127.0.0.1:{}/{}/system-builder@{}",
            self.options.port, self.options.namespace, component.root.digest
        );
        let dns = format!("{}.{}.svc", self.name("builder"), self.options.namespace);
        for (suffix, names) in [
            ("builder-server", vec!["ca.crt", "server.crt", "server.key"]),
            ("builder-client", vec!["ca.crt", "client.crt", "client.key"]),
        ] {
            let data: BTreeMap<_, _> = names
                .into_iter()
                .map(|n| (n, STANDARD.encode(&material[n])))
                .collect();
            self.owned(json!({"apiVersion":"v1","kind":"Secret","metadata":self.metadata(&self.name(suffix)),"type":"Opaque","data":data})).await?;
        }
        self.owned(json!({"apiVersion":"v1","kind":"ConfigMap","metadata":self.metadata(&self.name("builder-config")),"immutable":true,"data":{"buildkitd.toml":CONFIG}})).await?;
        let mut peers = vec![
            json!({"podSelector":{"matchLabels":{"kimage.dev/installation":self.options.installation,"kimage.dev/role":"builder-client"}}}),
        ];
        if let Some(app) = &self.options.application_namespace {
            peers.push(json!({"namespaceSelector":{"matchLabels":{"kubernetes.io/metadata.name":app}},"podSelector":{"matchLabels":{crate::build_inputs::INSTALLATION_LABEL:self.options.installation,"kimage.dev/role":"builder-client"}}}));
        }
        self.owned(json!({"apiVersion":"networking.k8s.io/v1","kind":"NetworkPolicy","metadata":self.metadata(&self.name("builder-isolation")),"spec":{"podSelector":{"matchLabels":{"kimage.dev/installation":self.options.installation,"kimage.dev/role":"builder"}},"policyTypes":["Ingress","Egress"],"egress":[],"ingress":[{"from":peers,"ports":[{"protocol":"TCP","port":1234}]}]}})).await?;
        let container = json!({"name":"builder","image":image,"imagePullPolicy":"IfNotPresent","command":["/kimage-builder"],"args":["--config","/configuration/buildkitd.toml"],
            "env":[{"name":"PATH","value":"/"},{"name":"OTEL_TRACES_EXPORTER","value":"none"}],
            "volumeMounts":[{"name":"cache","mountPath":"/cache"},{"name":"run","mountPath":"/run"},{"name":"tmp","mountPath":"/tmp"},{"name":"config","mountPath":"/configuration","readOnly":true},{"name":"credentials","mountPath":"/credentials","readOnly":true}],
            "resources":{"requests":{"cpu":"100m","memory":"128Mi"},"limits":{"cpu":"1","memory":"1Gi","ephemeral-storage":"5Gi"}},
            "securityContext":{"runAsUser":0,"runAsGroup":0,"privileged":false,"allowPrivilegeEscalation":false,"readOnlyRootFilesystem":true,"procMount":"Unmasked","seccompProfile":{"type":"Unconfined"},"appArmorProfile":{"type":"Unconfined"},"capabilities":{"drop":["ALL"],"add":WORKER_CAPABILITIES}},
            "readinessProbe":{"exec":{"command":["/buildctl","--addr","unix:///run/buildkit/buildkitd.sock","debug","workers"]},"periodSeconds":5,"timeoutSeconds":5}});
        let mut pod = self.pod("builder", vec![container], vec![
            json!({"name":"cache","emptyDir":{"sizeLimit":"4Gi"}}),
            json!({"name":"run","emptyDir":{"sizeLimit":"16Mi"}}),
            json!({"name":"tmp","emptyDir":{"sizeLimit":"256Mi"}}),
            json!({"name":"config","configMap":{"name":self.name("builder-config")}}),
            json!({"name":"credentials","secret":{"secretName":self.name("builder-server"),"defaultMode":256}}),
        ], false);
        pod["spec"]["hostUsers"] = json!(false);
        self.owned(json!({"apiVersion":"v1","kind":"ConfigMap","metadata":self.metadata(&self.name("builder-template")),"immutable":true,"data":{"pod.json":serde_json::to_string(&pod)?}})).await?;
        self.owned(pod).await?;
        self.owned(json!({"apiVersion":"v1","kind":"Service","metadata":self.metadata(&self.name("builder")),"spec":{"selector":{"kimage.dev/installation":self.options.installation,"kimage.dev/role":"builder"},"ports":[{"port":1234,"targetPort":1234}]}})).await?;
        self.wait_pod(&self.name("builder")).await.context("DefaultBuilderUnavailable: requires user namespaces, idmapped volumes and approved worker policy; no privileged fallback")?;
        self.wait_service_target(&self.name("builder"), &self.name("builder"))
            .await?;
        let mut metadata = self.metadata(&self.name("builder-check"));
        metadata["labels"]["kimage.dev/role"] = json!("builder-client");
        let check = json!({"apiVersion":"v1","kind":"Pod","metadata":metadata,"spec":{"automountServiceAccountToken":false,"restartPolicy":"OnFailure","nodeName":self.options.node,"containers":[{
            "name":"check","image":image,"imagePullPolicy":"IfNotPresent","command":["/buildctl"],"args":["--addr",format!("tcp://{dns}:1234"),"--tlsservername",dns,"--tlscacert","/credentials/ca.crt","--tlscert","/credentials/client.crt","--tlskey","/credentials/client.key","debug","workers"],
            "volumeMounts":[{"name":"credentials","mountPath":"/credentials","readOnly":true}],"resources":{"requests":{"cpu":"10m","memory":"16Mi"},"limits":{"cpu":"500m","memory":"128Mi"}},"securityContext":{"runAsUser":65532,"runAsGroup":65532,"runAsNonRoot":true,"allowPrivilegeEscalation":false,"readOnlyRootFilesystem":true,"capabilities":{"drop":["ALL"]}}}],"volumes":[{"name":"credentials","secret":{"secretName":self.name("builder-client"),"defaultMode":292}}]}});
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.options.namespace);
        if let Some(previous) = pods.get_opt(&self.name("builder-check")).await?
            && previous
                .spec
                .as_ref()
                .and_then(|s| s.restart_policy.as_deref())
                == Some("Never")
            && previous.status.as_ref().and_then(|s| s.phase.as_deref()) == Some("Failed")
        {
            // Migrate only the exact failed one-shot helper. Cluster network-policy
            // programming may lag Pod creation, so retry within the same Pod IP.
            let mut old = check.clone();
            old["spec"]["restartPolicy"] = json!("Never");
            self.owned(old).await?;
            self.remove_transition_pod("builder-check").await?;
        }
        self.owned(check.clone()).await?;
        if pods
            .get(&self.name("builder-check"))
            .await?
            .status
            .as_ref()
            .and_then(|s| s.phase.as_deref())
            == Some("Succeeded")
        {
            self.remove_transition_pod("builder-check").await?;
            self.owned(check).await?;
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            let pod = pods.get(&self.name("builder-check")).await?;
            let phase = pod.status.as_ref().and_then(|s| s.phase.as_deref());
            if phase == Some("Succeeded") {
                break;
            }
            ensure!(
                phase != Some("Failed"),
                "BuilderAuthenticatedReadinessFailed"
            );
            ensure!(
                tokio::time::Instant::now() < deadline,
                "BuilderAuthenticatedReadinessTimeout"
            );
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        // Catalog comes last: only an authenticated, ready worker is approved.
        let catalog = json!({"schema":1,"builders":{"default":{"engine":"BuildKit","version":"0.33.0","frontendVersion":"1.27.0","endpoint":format!("tcp://{dns}:1234"),"tlsServerName":dns,"clientSecret":self.name("builder-client"),"formats":["Containerfile"],"strategies":["Direct"],"platforms":["linux/amd64"],"node":self.options.node,"isolation":"PodUserNamespace","trustScope":self.options.application_namespace,"cache":{"modes":["Local","Disabled"],"maxUsedBytes":2147483648_u64,"volumeLimitBytes":4294967296_u64},"maxParallelism":1,"offline":true}}});
        self.owned(json!({"apiVersion":"v1","kind":"ConfigMap","metadata":self.metadata(&self.name("builders")),"immutable":true,"data":{"catalog.json":serde_json::to_string(&catalog)?}})).await?;
        self.state_named(bundle, "DefaultBuilderReady", "builder-state")
            .await
    }
}

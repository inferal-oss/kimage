//! Native resumable API-only bootstrap. No shell, node configuration or runtime imports.
use crate::{bundle::Bundle, registry::Registry, tunnel::Tunnel};
use anyhow::{Context, Result, bail, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use k8s_openapi::api::core::v1::{Node, Pod};
use kube::{
    Api, Client, ResourceExt,
    api::{ApiResource, DeleteParams, DynamicObject, ListParams, PostParams, Preconditions},
    core::GroupVersionKind,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{path::PathBuf, time::Duration};
pub const LABEL: &str = "kimage.dev/installation";
pub(crate) const REGISTRY_CONFIG: &str = "version: 0.1\nlog:\n  level: warn\nstorage:\n  filesystem:\n    rootdirectory: /data\n  delete:\n    enabled: false\nhttp:\n  addr: 127.0.0.1:5000\n  host: http://127.0.0.1:5000\n";
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Options {
    #[serde(default)]
    pub scope: InstallScope,
    #[serde(default)]
    pub excluded_namespaces: std::collections::BTreeSet<String>,
    #[serde(default)]
    pub application_namespace: Option<String>,
    #[serde(default)]
    pub runtime_profile: RuntimeProfile,
    pub namespace: String,
    pub installation: String,
    pub node: String,
    pub carrier: String,
    pub storage_class: String,
    pub pvc: Option<String>,
    pub port: u16,
    pub credentials: PathBuf,
}
#[derive(Clone, Copy, Default, Debug, Serialize, Deserialize, clap::ValueEnum, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum InstallScope {
    #[default]
    Namespace,
    Cluster,
}
#[derive(Clone, Copy, Default, Debug, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "camelCase")]
pub enum RuntimeProfile {
    #[default]
    Inferal,
    K3s,
}
pub struct Installer {
    pub client: Client,
    pub options: Options,
}
impl Installer {
    pub fn scope(&self) -> kimage_core::scope::NamespaceScope {
        kimage_core::scope::NamespaceScope {
            system_namespace: self.options.namespace.clone(),
            application_namespace: self.options.application_namespace.clone(),
            cluster_wide: self.options.scope == InstallScope::Cluster,
            excluded_namespaces: self.options.excluded_namespaces.clone(),
        }
    }
    pub fn new(client: Client, options: Options) -> Result<Self> {
        ensure!(
            kimage_core::selector::dns_label(&options.namespace)
                && kimage_core::selector::dns_label(&options.installation)
                && options.installation.len() <= 32,
            "InvalidInstallationScope"
        );
        if let Some(app) = &options.application_namespace {
            ensure!(
                kimage_core::selector::dns_label(app),
                "InvalidApplicationNamespace"
            );
        }
        ensure!(
            options.scope != InstallScope::Cluster || options.application_namespace.is_none(),
            "ClusterScopeApplicationNamespaceConflict"
        );
        ensure!(options.port >= 1024, "NonprivilegedPortRequired");
        ensure!(
            options
                .carrier
                .rsplit_once('@')
                .is_some_and(|(_, d)| kimage_core::oci::valid_digest(d)),
            "ExplicitCarrierDigestRequired"
        );
        Ok(Self { client, options })
    }
    pub(crate) fn name(&self, suffix: &str) -> String {
        format!("{}-{suffix}", self.options.installation)
    }
    pub(crate) fn metadata(&self, name: &str) -> Value {
        json!({"name":name,"labels":{LABEL:self.options.installation}})
    }
    pub(crate) fn api_at(&self, kind: &str, namespace: Option<&str>) -> Result<Api<DynamicObject>> {
        let (group, version, plural) = match kind {
            "Pod" => ("", "v1", "pods"),
            "ConfigMap" => ("", "v1", "configmaps"),
            "Secret" => ("", "v1", "secrets"),
            "Service" => ("", "v1", "services"),
            "ServiceAccount" => ("", "v1", "serviceaccounts"),
            "NetworkPolicy" => ("networking.k8s.io", "v1", "networkpolicies"),
            "PersistentVolumeClaim" => ("", "v1", "persistentvolumeclaims"),
            "Role" => ("rbac.authorization.k8s.io", "v1", "roles"),
            "RoleBinding" => ("rbac.authorization.k8s.io", "v1", "rolebindings"),
            "ClusterRole" => ("rbac.authorization.k8s.io", "v1", "clusterroles"),
            "ClusterRoleBinding" => ("rbac.authorization.k8s.io", "v1", "clusterrolebindings"),
            "MutatingWebhookConfiguration" => (
                "admissionregistration.k8s.io",
                "v1",
                "mutatingwebhookconfigurations",
            ),
            "ValidatingWebhookConfiguration" => (
                "admissionregistration.k8s.io",
                "v1",
                "validatingwebhookconfigurations",
            ),
            _ => bail!("UnsupportedInstallerResource"),
        };
        let mut ar = ApiResource::from_gvk(&GroupVersionKind::gvk(group, version, kind));
        ar.plural = plural.into();
        Ok(match namespace {
            Some(ns) => Api::namespaced_with(self.client.clone(), ns, &ar),
            None => Api::all_with(self.client.clone(), &ar),
        })
    }
    fn api(&self, kind: &str) -> Result<Api<DynamicObject>> {
        self.api_at(kind, Some(&self.options.namespace))
    }
    pub(crate) async fn owned(&self, v: Value) -> Result<()> {
        self.owned_at(v, Some(&self.options.namespace)).await
    }
    pub(crate) async fn owned_at(&self, v: Value, namespace: Option<&str>) -> Result<()> {
        let kind = v["kind"].as_str().context("MissingKind")?;
        let name = v["metadata"]["name"].as_str().context("MissingName")?;
        let api = self.api_at(kind, namespace)?;
        if let Some(existing) = api.get_opt(name).await? {
            ensure!(
                existing.labels().get(LABEL) == Some(&self.options.installation),
                "ForeignResourceConflict: {kind}/{name}"
            );
            let mut actual = serde_json::to_value(existing)?;
            if kind == "Pod" {
                normalize_serviceaccount_projection(&mut actual["spec"], &v["spec"])?;
            }
            if kind == "NetworkPolicy" {
                normalize_empty_network_rules(&mut actual["spec"], &v["spec"]);
            }
            for field in [
                "spec",
                "data",
                "binaryData",
                "immutable",
                "type",
                "rules",
                "subjects",
                "roleRef",
                "webhooks",
                "automountServiceAccountToken",
            ] {
                if let Some(expected) = v.get(field) {
                    ensure!(
                        contains(&actual[field], expected),
                        "ExistingResourceConflict: {kind}/{name} {}",
                        difference(&actual[field], expected, &format!("/{field}"))
                            .unwrap_or_default()
                    );
                }
            }
        } else {
            let desired: DynamicObject = serde_json::from_value(v)?;
            api.create(
                &PostParams {
                    dry_run: true,
                    ..Default::default()
                },
                &desired,
            )
            .await?;
            api.create(&PostParams::default(), &desired).await?;
        }
        Ok(())
    }
    pub async fn node_platform(client: Client, node: &str) -> Result<kimage_core::oci::Platform> {
        let node: Node = Api::all(client).get(node).await?;
        let info = node
            .status
            .and_then(|s| s.node_info)
            .context("MissingNodeInfo")?;
        let platform = kimage_core::oci::Platform {
            os: info.operating_system,
            architecture: info.architecture,
        };
        ensure!(platform.supported(), "UnsupportedNodePlatform");
        Ok(platform)
    }
    pub async fn preflight_bundle(&self, bundle: &Bundle) -> Result<()> {
        self.preflight().await?;
        ensure!(
            Self::node_platform(self.client.clone(), &self.options.node).await?
                == bundle.inventory.platform,
            "BundleNodePlatformMismatch"
        );
        Ok(())
    }
    pub async fn preflight(&self) -> Result<()> {
        let nodes: Api<Node> = Api::all(self.client.clone());
        let node = nodes.get(&self.options.node).await?;
        let status = node.status.context("MissingNodeStatus")?;
        let info = status.node_info.context("MissingNodeInfo")?;
        ensure!(
            kimage_core::oci::Platform {
                os: info.operating_system.clone(),
                architecture: info.architecture.clone()
            }
            .supported(),
            "UnsupportedNodePlatform"
        );
        ensure!(
            match self.options.runtime_profile {
                RuntimeProfile::Inferal =>
                    info.kubelet_version == "v1.36.5+rke2r1"
                        && info.container_runtime_version == "containerd://2.3.4-k3s1",
                RuntimeProfile::K3s =>
                    info.kubelet_version == "v1.36.5+k3s1"
                        && info.container_runtime_version == "containerd://2.3.4-k3s1.36",
            },
            "UntestedRuntime"
        );
        ensure!(
            status
                .conditions
                .as_ref()
                .is_some_and(|cs| cs.iter().any(|c| c.type_ == "Ready" && c.status == "True")),
            "NodeNotReady"
        );
        ensure!(
            status.images.as_ref().is_some_and(|is| is.iter().any(|i| i
                .names
                .as_ref()
                .is_some_and(|ns| ns.contains(&self.options.carrier)))),
            "CarrierNotReportedCached"
        );
        for file in [
            "ca.crt",
            "tls.crt",
            "tls.key",
            "reader-password",
            "publisher-password",
        ] {
            let path = self.options.credentials.join(file);
            let meta = std::fs::symlink_metadata(path)?;
            ensure!(
                meta.is_file() && meta.len() > 0 && meta.len() <= 65536,
                "InvalidCredentialFile"
            );
        }
        ensure!(
            distinct_passwords(
                &std::fs::read_to_string(self.options.credentials.join("reader-password"))?,
                &std::fs::read_to_string(self.options.credentials.join("publisher-password"))?
            ),
            "IndependentRegistryCredentialsRequired"
        );
        Ok(())
    }
    async fn payload(&self, bundle: &Bundle, role: &str) -> Result<(Value, usize, String)> {
        self.payload_scoped(bundle, role, &self.options.installation)
            .await
    }
    pub(crate) async fn payload_scoped(
        &self,
        bundle: &Bundle,
        role: &str,
        scope: &str,
    ) -> Result<(Value, usize, String)> {
        let (chunks, digest) = bundle.chunks(role)?;
        let mut sources = Vec::new();
        for (i, chunk) in chunks.iter().enumerate() {
            let name = format!("{scope}-{role}-{i:04}");
            let key = format!("{i:04}");
            let value = json!({"apiVersion":"v1","kind":"ConfigMap","metadata":self.metadata(&name),"immutable":true,"binaryData":{key:STANDARD.encode(chunk)}});
            ensure!(
                serde_json::to_vec(&value)?.len() <= 786432,
                "EncodedPayloadTooLarge"
            );
            self.owned(value).await?;
            sources.push(json!({"configMap":{"name":name}}));
        }
        Ok((
            json!({"name":format!("{role}-payload"),"projected":{"sources":sources}}),
            chunks.len(),
            digest,
        ))
    }
    pub(crate) async fn state(&self, bundle: &Bundle, phase: &str) -> Result<()> {
        self.state_named(bundle, phase, "state").await
    }
    pub(crate) async fn state_named(
        &self,
        bundle: &Bundle,
        phase: &str,
        suffix: &str,
    ) -> Result<()> {
        let api = self.api("ConfigMap")?;
        let name = self.name(suffix);
        let config = self.configuration(&bundle.digest)?;
        let v = json!({"apiVersion":"v1","kind":"ConfigMap","metadata":self.metadata(&name),"data":{"configuration":config,"phase":phase,"bundle":bundle.digest}});
        let mut desired: DynamicObject = serde_json::from_value(v)?;
        if let Some(existing) = api.get_opt(&name).await? {
            ensure!(
                existing.labels().get(LABEL) == Some(&self.options.installation)
                    && existing.data["data"]["configuration"] == config,
                "InstallationConfigurationConflict"
            );
            desired.metadata.resource_version = existing.resource_version();
            api.replace(&name, &PostParams::default(), &desired).await?;
        } else {
            api.create(&PostParams::default(), &desired).await?;
        }
        Ok(())
    }
    pub(crate) fn configuration(&self, bundle: &str) -> Result<String> {
        Ok(kimage_core::oci::digest(&serde_json::to_vec(
            &json!({"namespace":self.options.namespace,"installation":self.options.installation,"node":self.options.node,"carrier":self.options.carrier,"storageClass":self.options.storage_class,"pvc":self.options.pvc,"port":self.options.port,"bundle":bundle,"applicationNamespace":self.options.application_namespace,"scope":self.options.scope,"excludedNamespaces":self.options.excluded_namespaces,"runtimeProfile":self.options.runtime_profile}),
        )?))
    }
    pub async fn bootstrap(&self, bundle: &Bundle) -> Result<()> {
        self.preflight_bundle(bundle).await?;
        crate::credentials::validate_registry_leaf(
            format!("{}.{}.svc", self.name("registry"), self.options.namespace),
            &self.options.credentials,
        )
        .await?;
        self.ensure_namespaces().await?;
        self.bootstrap_policy_preflight().await?;
        self.state(bundle, "Preparing").await?;
        let mut credentials = serde_json::Map::new();
        for f in [
            "ca.crt",
            "tls.crt",
            "tls.key",
            "reader-password",
            "publisher-password",
        ] {
            credentials.insert(
                f.into(),
                json!(STANDARD.encode(std::fs::read(self.options.credentials.join(f))?)),
            );
        }
        self.owned(json!({"apiVersion":"v1","kind":"Secret","metadata":self.metadata(&self.name("registry-credentials")),"type":"Opaque","data":credentials})).await?;
        self.owned(json!({"apiVersion":"v1","kind":"Secret","metadata":self.metadata(&self.name("proxy-credentials")),"type":"Opaque","data":{"ca.crt":STANDARD.encode(std::fs::read(self.options.credentials.join("ca.crt"))?),"reader-password":STANDARD.encode(std::fs::read(self.options.credentials.join("reader-password"))?)}})).await?;
        self.owned(json!({"apiVersion":"v1","kind":"ConfigMap","metadata":self.metadata(&self.name("registry-config")),"immutable":true,"data":{"config.yml":REGISTRY_CONFIG}})).await?;
        if let Some(name) = &self.options.pvc {
            let pvcs = self.api("PersistentVolumeClaim")?;
            let pvc = pvcs.get(name).await?;
            ensure!(
                pvc.data["status"]["phase"] == "Bound",
                "ExistingPvcNotBound"
            );
        } else {
            self.owned(json!({"apiVersion":"v1","kind":"PersistentVolumeClaim","metadata":self.metadata(&self.name("data")),"spec":{"accessModes":["ReadWriteOnce"],"storageClassName":self.options.storage_class,"resources":{"requests":{"storage":"1Gi"}}}})).await?;
        }
        let launcher = json!({"apiVersion":"v1","kind":"ConfigMap","metadata":self.metadata(&self.name("launcher")),"immutable":true,"binaryData":{"launcher":STANDARD.encode(std::fs::read(bundle.path(&bundle.inventory.launcher)?)?)}});
        ensure!(
            serde_json::to_vec(&launcher)?.len() <= 786432,
            "EncodedLauncherTooLarge"
        );
        self.owned(launcher).await?;
        self.owned(json!({"apiVersion":"v1","kind":"Service","metadata":self.metadata(&self.name("registry")),"spec":{"selector":{LABEL:self.options.installation,"kimage.dev/role":"registry"},"ports":[{"port":5443,"targetPort":5443}]}})).await?;
        let mut registry_volumes = vec![
            json!({"name":"launcher","configMap":{"name":self.name("launcher"),"defaultMode":365}}),
            json!({"name":"work","emptyDir":{"sizeLimit":"256Mi"}}),
            json!({"name":"config","configMap":{"name":self.name("registry-config")}}),
            json!({"name":"credentials","secret":{"secretName":self.name("registry-credentials"),"defaultMode":256}}),
            json!({"name":"data","persistentVolumeClaim":{"claimName":self.options.pvc.clone().unwrap_or_else(||self.name("data"))}}),
        ];
        let mut containers = Vec::new();
        for role in ["registry", "gateway"] {
            let (volume, count, digest) = self.payload(bundle, role).await?;
            registry_volumes.push(volume);
            let mut args = vec![
                format!("/payload/{role}"),
                count.to_string(),
                digest,
                format!("/work/{role}"),
            ];
            let mut mounts = vec![
                json!({"name":"launcher","mountPath":"/bootstrap","readOnly":true}),
                json!({"name":format!("{role}-payload"),"mountPath":format!("/payload/{role}"),"readOnly":true}),
                json!({"name":"work","mountPath":"/work"}),
            ];
            if role == "registry" {
                args.extend(["serve".into(), "/config/config.yml".into()]);
                mounts.extend([
                    json!({"name":"config","mountPath":"/config","readOnly":true}),
                    json!({"name":"data","mountPath":"/data"}),
                ]);
            } else {
                args.extend([
                    "--cert".into(),
                    "/credentials/tls.crt".into(),
                    "--key".into(),
                    "/credentials/tls.key".into(),
                    "--read-password".into(),
                    "/credentials/reader-password".into(),
                    "--write-password".into(),
                    "/credentials/publisher-password".into(),
                    "--namespaces".into(),
                    self.allowed_namespaces(),
                ]);
                mounts
                    .push(json!({"name":"credentials","mountPath":"/credentials","readOnly":true}));
            }
            containers.push(self.container(role, args, mounts));
        }
        self.owned(self.pod("registry", containers, registry_volumes, false))
            .await?;
        let (payload, count, digest) = self.payload(bundle, "proxy").await?;
        let args = vec![
            "/payload/proxy".into(),
            count.to_string(),
            digest,
            "/work/proxy".into(),
            "--listen".into(),
            format!("127.0.0.1:{}", self.options.port),
            "--upstream".into(),
            self.origin(),
            "--ca".into(),
            "/credentials/ca.crt".into(),
            "--password-file".into(),
            "/credentials/reader-password".into(),
            "--namespaces".into(),
            self.allowed_namespaces(),
        ];
        let mounts = vec![
            json!({"name":"launcher","mountPath":"/bootstrap","readOnly":true}),
            json!({"name":"proxy-payload","mountPath":"/payload/proxy","readOnly":true}),
            json!({"name":"work","mountPath":"/work"}),
            json!({"name":"credentials","mountPath":"/credentials","readOnly":true}),
        ];
        let volumes = vec![
            payload,
            json!({"name":"launcher","configMap":{"name":self.name("launcher"),"defaultMode":365}}),
            json!({"name":"work","emptyDir":{"sizeLimit":"128Mi"}}),
            json!({"name":"credentials","secret":{"secretName":self.name("proxy-credentials"),"defaultMode":256}}),
        ];
        let mut proxy_pod = self.pod(
            "proxy",
            vec![self.container("proxy", args, mounts)],
            volumes,
            true,
        );
        let node: Node = Api::all(self.client.clone())
            .get(&self.options.node)
            .await?;
        proxy_pod["metadata"]["annotations"]["kimage.dev/node-uid"] =
            json!(node.uid().context("MissingNodeUid")?);
        proxy_pod["spec"]["containers"][0]["readinessProbe"] = json!({"httpGet":{"host":"127.0.0.1","path":"/v2/","port":self.options.port},"periodSeconds":2,"timeoutSeconds":3});
        self.owned(proxy_pod).await?;
        self.wait_pod(&self.name("registry")).await?;
        self.wait_pod(&self.name("proxy")).await?;
        self.state(bundle, "CarrierServing").await
    }
    pub(crate) fn container(&self, role: &str, args: Vec<String>, mounts: Vec<Value>) -> Value {
        json!({"name":role,"image":self.options.carrier,"imagePullPolicy":"Never","command":["/bootstrap/launcher"],"args":args,"env":[{"name":"OTEL_TRACES_EXPORTER","value":"none"}],"volumeMounts":mounts,"resources":{"requests":{"cpu":"25m","memory":"64Mi"},"limits":{"cpu":"1","memory":"256Mi","ephemeral-storage":"256Mi"}},"securityContext":{"runAsUser":0,"runAsGroup":0,"allowPrivilegeEscalation":false,"readOnlyRootFilesystem":true,"capabilities":{"drop":["ALL"]}}})
    }
    pub(crate) fn pod(
        &self,
        role: &str,
        containers: Vec<Value>,
        volumes: Vec<Value>,
        host: bool,
    ) -> Value {
        let mut metadata = self.metadata(&self.name(role));
        metadata["labels"]["kimage.dev/role"] = json!(role);
        let mut pod = json!({"apiVersion":"v1","kind":"Pod","metadata":metadata,"spec":{"automountServiceAccountToken":false,"restartPolicy":"Always","hostNetwork":host,"dnsPolicy":if host{"ClusterFirstWithHostNet"}else{"ClusterFirst"},"securityContext":{"seccompProfile":{"type":"RuntimeDefault"}},"containers":containers,"volumes":volumes}});
        if host {
            pod["spec"]["nodeName"] = json!(self.options.node);
        } else {
            pod["spec"]["affinity"] = json!({"nodeAffinity":{"requiredDuringSchedulingIgnoredDuringExecution":{"nodeSelectorTerms":[{"matchFields":[{"key":"metadata.name","operator":"In","values":[self.options.node]}]}]}}});
        }
        pod
    }
    pub(crate) fn allowed_namespaces(&self) -> String {
        self.scope()
            .registry_namespaces()
            .into_iter()
            .collect::<Vec<_>>()
            .join(",")
    }
    pub(crate) fn origin(&self) -> String {
        format!(
            "https://{}.{}.svc:5443/",
            self.name("registry"),
            self.options.namespace
        )
    }
    pub(crate) async fn wait_pod(&self, name: &str) -> Result<()> {
        let api: Api<Pod> = Api::namespaced(self.client.clone(), &self.options.namespace);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
        loop {
            let pod = api.get(name).await?;
            if pod
                .status
                .as_ref()
                .and_then(|s| s.conditions.as_ref())
                .is_some_and(|cs| cs.iter().any(|c| c.type_ == "Ready" && c.status == "True"))
            {
                return Ok(());
            }
            ensure!(
                tokio::time::Instant::now() < deadline,
                "BootstrapTimeout: inspect carrier/admission/PVC/port diagnostics"
            );
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
    pub async fn seed(&self, bundle: &Bundle) -> Result<()> {
        let state = self.api("ConfigMap")?.get(&self.name("state")).await?;
        let previous = state.data["data"]["bundle"]
            .as_str()
            .context("MissingInstallationBundle")?;
        ensure!(
            state.labels().get(LABEL) == Some(&self.options.installation)
                && state.data["data"]["configuration"] == self.configuration(previous)?,
            "InstallationConfigurationConflict"
        );
        // Staging immutable component roots never rewrites active serving refs.
        let journal = format!("seed-{}", &bundle.digest[7..19]);
        self.state_named(bundle, "ComponentsPending", &journal)
            .await?;
        let tunnel = Tunnel::open(
            self.client.clone(),
            &self.options.namespace,
            &self.name("registry"),
            5443,
        )
        .await?;
        let ca = std::fs::read(self.options.credentials.join("ca.crt"))?;
        let password =
            std::fs::read_to_string(self.options.credentials.join("publisher-password"))?;
        for (name, component) in &bundle.inventory.components {
            let verified = kimage_core::oci::verify_archive(
                &bundle.path(&component.archive)?,
                &component.root,
                &bundle.inventory.platform,
                kimage_core::oci::Limits::default(),
            )?;
            let registry = Registry::new(
                self.origin().parse()?,
                format!("{}/system-{name}", self.options.namespace),
                &ca,
                Some(tunnel.address),
                "publisher".into(),
                password.trim_end().into(),
            )?;
            registry
                .publish(&verified, &tokio_util::sync::CancellationToken::new())
                .await?;
        }
        self.state_named(bundle, "ComponentsVerified", &journal)
            .await?;
        if previous == bundle.digest {
            self.state(bundle, "ComponentsVerified").await?;
        }
        Ok(())
    }
    /// Warm replacements, maintain a temporary read-only registry and overlap
    /// compatible loopback proxies. Publication pauses while the writer changes.
    pub async fn handover(&self, bundle: &Bundle) -> Result<()> {
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.options.namespace);
        for (role, component) in &bundle.inventory.components {
            let binary = if role == "registry" {
                "/registry".into()
            } else {
                format!("/kimage-{role}")
            };
            let reference = format!(
                "127.0.0.1:{}/{}/system-{role}@{}",
                self.options.port, self.options.namespace, component.root.digest
            );
            let name = self.name(&format!("warm-{role}"));
            self.owned(json!({"apiVersion":"v1","kind":"Pod","metadata":self.metadata(&name),"spec":{"nodeName":self.options.node,"automountServiceAccountToken":false,"restartPolicy":"Never","containers":[{"name":"warmup","image":reference,"imagePullPolicy":"IfNotPresent","command":[binary,"--help"],"securityContext":{"allowPrivilegeEscalation":false,"readOnlyRootFilesystem":true,"capabilities":{"drop":["ALL"]}},"resources":{"requests":{"cpu":"10m","memory":"16Mi"},"limits":{"cpu":"500m","memory":"128Mi"}}}]}})).await?;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
            loop {
                let pod = pods.get(&name).await?;
                let phase = pod.status.as_ref().and_then(|s| s.phase.as_deref());
                if phase == Some("Succeeded") {
                    break;
                }
                ensure!(phase != Some("Failed"), "ComponentWarmupFailed: {role}");
                ensure!(
                    tokio::time::Instant::now() < deadline,
                    "ComponentWarmupTimeout: {role}"
                );
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
        self.state(bundle, "ReplacementsWarmed").await?;
        self.prepare_readthrough(bundle).await?;
        for role in ["registry", "proxy"] {
            let name = self.name(role);
            let checkpoint_name = self.name(&format!("replacement-{role}"));
            let Some(existing) = pods.get_opt(&name).await? else {
                let api = self.api("ConfigMap")?;
                let checkpoint = api
                    .get(&checkpoint_name)
                    .await
                    .context("MissingHandoverCheckpoint: recover the carrier serving path first")?;
                ensure!(
                    checkpoint.labels().get(LABEL) == Some(&self.options.installation)
                        && checkpoint.data["data"]["bundle"] == bundle.digest,
                    "HandoverCheckpointConflict"
                );
                let saved: Value = serde_json::from_str(
                    checkpoint.data["data"]["pod"]
                        .as_str()
                        .context("InvalidHandoverCheckpoint")?,
                )?;
                ensure!(
                    saved["kind"] == "Pod"
                        && saved["metadata"]["name"] == name
                        && saved["metadata"]["labels"][LABEL] == self.options.installation,
                    "InvalidHandoverCheckpoint"
                );
                self.owned(saved).await?;
                self.wait_pod(&name).await?;
                if role == "registry" {
                    self.registry_route("registry").await?;
                }
                if role == "proxy" {
                    self.remove_transition_pod("proxy-next").await?;
                }
                self.state(
                    bundle,
                    if role == "registry" {
                        "OrdinaryRegistry"
                    } else {
                        "OrdinaryServing"
                    },
                )
                .await?;
                continue;
            };
            ensure!(
                existing.labels().get(LABEL) == Some(&self.options.installation),
                "ServingOwnershipConflict"
            );
            let mut replacement = existing.clone();
            let spec = replacement.spec.as_mut().context("MissingServingSpec")?;
            let ordinary = spec.containers.iter().all(|c| {
                c.image
                    .as_ref()
                    .is_some_and(|i| i.starts_with(&format!("127.0.0.1:{}/", self.options.port)))
            });
            if ordinary {
                for c in &spec.containers {
                    let component = bundle
                        .inventory
                        .components
                        .get(&c.name)
                        .context("UnexpectedServingComponent")?;
                    ensure!(
                        c.image.as_deref()
                            == Some(&format!(
                                "127.0.0.1:{}/{}/system-{}@{}",
                                self.options.port,
                                self.options.namespace,
                                c.name,
                                component.root.digest
                            )),
                        "ServingBundleConflict"
                    );
                }
                self.wait_pod(&name).await?;
                if role == "registry" {
                    self.registry_route("registry").await?;
                }
                continue;
            }
            for container in &mut spec.containers {
                ensure!(
                    container.image.as_deref() == Some(&self.options.carrier)
                        && container
                            .command
                            .as_ref()
                            .is_some_and(|c| c == &["/bootstrap/launcher"]),
                    "UnexpectedServingDefinition"
                );
                let component = bundle
                    .inventory
                    .components
                    .get(&container.name)
                    .context("MissingServingComponent")?;
                container.image = Some(format!(
                    "127.0.0.1:{}/{}/system-{}@{}",
                    self.options.port,
                    self.options.namespace,
                    container.name,
                    component.root.digest
                ));
                container.image_pull_policy = Some("IfNotPresent".into());
                container.command = Some(vec![if container.name == "registry" {
                    "/registry".into()
                } else {
                    format!("/kimage-{}", container.name)
                }]);
                let arguments = container
                    .args
                    .as_ref()
                    .context("MissingLauncherArguments")?;
                ensure!(arguments.len() >= 4, "InvalidLauncherArguments");
                container.args = Some(arguments[4..].to_vec());
                if let Some(mounts) = &mut container.volume_mounts {
                    mounts.retain(|m| {
                        m.name != "launcher" && m.name != "work" && !m.name.ends_with("-payload")
                    });
                }
            }
            if let Some(volumes) = &mut spec.volumes {
                volumes.retain(|v| {
                    v.name != "launcher" && v.name != "work" && !v.name.ends_with("-payload")
                });
            }
            let mut metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta =
                serde_json::from_value(self.metadata(&name))?;
            metadata
                .labels
                .get_or_insert_default()
                .insert("kimage.dev/role".into(), role.into());
            metadata.annotations = replacement.metadata.annotations.clone();
            replacement.metadata = metadata;
            replacement.status = None;
            if role == "proxy" {
                let args = replacement
                    .spec
                    .as_mut()
                    .context("MissingProxySpec")?
                    .containers[0]
                    .args
                    .get_or_insert_default();
                if !args.iter().any(|a| a == "--overlap") {
                    args.push("--overlap".into());
                }
            }
            let saved = serde_json::to_string(&replacement)?;
            ensure!(saved.len() <= 65536, "HandoverCheckpointTooLarge");
            self.owned(json!({"apiVersion":"v1","kind":"ConfigMap","metadata":self.metadata(&checkpoint_name),"immutable":true,"data":{"bundle":bundle.digest,"pod":saved}})).await?;
            if role == "proxy" {
                self.prepare_proxy_overlap(&replacement).await?;
            }
            if role == "registry" {
                self.registry_route("readthrough").await?;
            }
            pods.delete(
                &name,
                &DeleteParams {
                    preconditions: Some(Preconditions {
                        uid: existing.uid(),
                        resource_version: existing.resource_version(),
                    }),
                    ..Default::default()
                },
            )
            .await?;
            self.wait_absent(&name).await?;
            self.owned(serde_json::to_value(replacement)?).await?;
            self.wait_pod(&name).await?;
            if role == "registry" {
                self.registry_route("registry").await?;
            }
            if role == "proxy" {
                self.remove_transition_pod("proxy-next").await?;
            }
            self.state(
                bundle,
                if role == "registry" {
                    "OrdinaryRegistry"
                } else {
                    "OrdinaryServing"
                },
            )
            .await?;
        }
        self.registry_route("registry").await?;
        self.remove_transition_pod("readthrough").await?;
        self.cleanup_bootstrap(bundle).await?;
        Ok(())
    }
    pub(crate) async fn wait_absent(&self, name: &str) -> Result<()> {
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.options.namespace);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
        while pods.get_opt(name).await?.is_some() {
            ensure!(
                tokio::time::Instant::now() < deadline,
                "ServingTerminationTimeout"
            );
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        Ok(())
    }
    /// Only labelled disposable resources, with UID/resourceVersion delete preconditions.
    /// PVC/CRDs/custom resources are excluded, including when reusing a foreign PVC.
    pub async fn uninstall(&self) -> Result<()> {
        self.delete_owned("MutatingWebhookConfiguration", None)
            .await?;
        self.delete_owned("ValidatingWebhookConfiguration", None)
            .await?;
        // Stop the supervisor before deleting its worker; otherwise an active
        // build observer can recreate the worker during the uninstall scan.
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.options.namespace);
        if let Some(operator) = pods.get_opt(&self.name("operator")).await? {
            ensure!(
                operator.labels().get(LABEL) == Some(&self.options.installation),
                "OperatorOwnershipConflict"
            );
            pods.delete(
                &self.name("operator"),
                &DeleteParams {
                    preconditions: Some(Preconditions {
                        uid: operator.uid(),
                        resource_version: operator.resource_version(),
                    }),
                    ..Default::default()
                },
            )
            .await?;
            self.wait_absent(&self.name("operator")).await?;
        }
        // Stop controllers and the shared worker before deleting application
        // sessions; a disconnected build RPC cannot outlive its worker.
        self.delete_owned("Pod", Some(&self.options.namespace))
            .await?;
        self.cleanup_build_sessions().await?;
        for kind in [
            "Service",
            "NetworkPolicy",
            "ConfigMap",
            "Secret",
            "RoleBinding",
            "Role",
            "ServiceAccount",
        ] {
            self.delete_owned(kind, Some(&self.options.namespace))
                .await?;
        }
        if let Some(app) = &self.options.application_namespace
            && app != &self.options.namespace
        {
            for kind in ["RoleBinding", "Role"] {
                self.delete_owned(kind, Some(app)).await?;
            }
        }
        for kind in ["ClusterRoleBinding", "ClusterRole"] {
            self.delete_owned(kind, None).await?;
        }
        Ok(())
    }
    async fn cleanup_build_sessions(&self) -> Result<()> {
        let app = self
            .options
            .application_namespace
            .as_deref()
            .unwrap_or(&self.options.namespace);
        let selector = format!(
            "{}={}",
            crate::build_inputs::INSTALLATION_LABEL,
            self.options.installation
        );
        for kind in ["Pod", "ConfigMap", "Secret"] {
            let api = self.api_at(
                kind,
                if self.scope().cluster_wide {
                    None
                } else {
                    Some(app)
                },
            )?;
            let page = api
                .list(&ListParams::default().labels(&selector).limit(512))
                .await?;
            ensure!(
                page.metadata
                    .continue_
                    .as_ref()
                    .is_none_or(|s| s.is_empty()),
                "TooManyBuildSessions"
            );
            for item in page {
                let namespace = item.namespace().context("MissingBuildSessionNamespace")?;
                ensure!(self.scope().allows(&namespace), "BuildSessionScopeMismatch");
                let api = self.api_at(kind, Some(&namespace))?;
                let id = item
                    .labels()
                    .get(crate::build_inputs::SNAPSHOT_LABEL)
                    .context("MissingBuildSnapshotId")?;
                ensure!(
                    id.len() == 40
                        && id.bytes().all(|b| b.is_ascii_hexdigit())
                        && item
                            .name_any()
                            .starts_with(&format!("build-{}-", &id[..20]))
                        && item.owner_references().iter().any(|o| o.kind == "Image"
                            && Some(&o.uid) == item.labels().get("kimage.dev/image-uid")),
                    "BuildSessionCleanupOwnershipConflict"
                );
                api.delete(
                    &item.name_any(),
                    &DeleteParams {
                        preconditions: Some(Preconditions {
                            uid: item.uid(),
                            resource_version: item.resource_version(),
                        }),
                        ..Default::default()
                    },
                )
                .await?;
                if kind == "Pod" {
                    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
                    while api.get_opt(&item.name_any()).await?.is_some() {
                        ensure!(
                            tokio::time::Instant::now() < deadline,
                            "BuildClientTerminationTimeout"
                        );
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                }
            }
        }
        Ok(())
    }
    async fn delete_owned(&self, kind: &str, namespace: Option<&str>) -> Result<()> {
        let api = self.api_at(kind, namespace)?;
        let items = api
            .list(&ListParams::default().labels(&format!("{LABEL}={}", self.options.installation)))
            .await?;
        for item in items {
            ensure!(
                item.name_any()
                    .starts_with(&format!("{}-", self.options.installation)),
                "UnexpectedOwnedResourceName"
            );
            api.delete(
                &item.name_any(),
                &DeleteParams {
                    preconditions: Some(Preconditions {
                        uid: item.uid(),
                        resource_version: item.resource_version(),
                    }),
                    ..Default::default()
                },
            )
            .await?;
            if kind == "Pod" {
                self.wait_absent(&item.name_any()).await?;
            }
        }
        Ok(())
    }
}
fn normalize_empty_network_rules(actual: &mut Value, expected: &Value) {
    // Kubernetes omits empty ingress/egress arrays. Their meaning is preserved
    // by the explicit policyTypes comparison, never by ignoring nonempty rules.
    for field in ["ingress", "egress"] {
        if expected[field] == json!([]) && actual[field].is_null() {
            actual[field] = json!([]);
        }
    }
}
fn distinct_passwords(reader: &str, publisher: &str) -> bool {
    let reader = reader.trim_end();
    let publisher = publisher.trim_end();
    !reader.is_empty()
        && !publisher.is_empty()
        && reader.len() <= 1024
        && publisher.len() <= 1024
        && reader != publisher
}
fn serviceaccount_projection() -> Value {
    json!({"defaultMode":420,"sources":[
        {"serviceAccountToken":{"expirationSeconds":3607,"path":"token"}},
        {"configMap":{"name":"kube-root-ca.crt","items":[{"key":"ca.crt","path":"ca.crt"}]}},
        {"downwardAPI":{"items":[{"path":"namespace","fieldRef":{"apiVersion":"v1","fieldPath":"metadata.namespace"}}]}}
    ]})
}
/// The tested API server appends this one default projection when automount is
/// explicitly requested. Never ignore arbitrary added volumes or mounts.
fn normalize_serviceaccount_projection(actual: &mut Value, expected: &Value) -> Result<()> {
    if actual.get("hostNetwork").is_none() && expected["hostNetwork"] == false {
        actual["hostNetwork"] = json!(false);
    }
    if expected["automountServiceAccountToken"] != true {
        return Ok(());
    }
    let volumes = actual["volumes"].as_array().context("MissingPodVolumes")?;
    let injected: Vec<_> = volumes
        .iter()
        .filter(|v| {
            v["name"]
                .as_str()
                .is_some_and(|n| n.starts_with("kube-api-access-"))
        })
        .collect();
    if injected.is_empty() {
        return Ok(());
    }
    ensure!(
        injected.len() == 1 && injected[0]["projected"] == serviceaccount_projection(),
        "UnexpectedServiceAccountProjection"
    );
    let name = injected[0]["name"]
        .as_str()
        .context("MissingProjectionName")?
        .to_owned();
    let mount = json!({"name":name,"mountPath":"/var/run/secrets/kubernetes.io/serviceaccount","readOnly":true});
    for kind in ["containers", "initContainers"] {
        if let Some(containers) = actual[kind].as_array_mut() {
            for container in containers {
                let mounts = container["volumeMounts"]
                    .as_array_mut()
                    .context("MissingCredentialMount")?;
                ensure!(
                    mounts.iter().filter(|m| *m == &mount).count() == 1,
                    "UnexpectedServiceAccountMount"
                );
                mounts.retain(|m| m != &mount);
            }
        }
    }
    actual["volumes"]
        .as_array_mut()
        .unwrap()
        .retain(|v| v["name"] != name);
    Ok(())
}

fn difference(actual: &Value, expected: &Value, path: &str) -> Option<String> {
    match expected {
        Value::Object(m) => m
            .iter()
            .find_map(|(k, v)| difference(&actual[k], v, &format!("{path}/{k}"))),
        Value::Array(a) => match actual.as_array() {
            Some(b) if a.len() == b.len() => a
                .iter()
                .zip(b)
                .enumerate()
                .find_map(|(i, (e, a))| difference(a, e, &format!("{path}/{i}"))),
            _ => Some(format!("{path}/length")),
        },
        _ => (actual != expected).then(|| path.to_owned()),
    }
}

fn contains(actual: &Value, expected: &Value) -> bool {
    match expected {
        Value::Object(m) => m.iter().all(|(k, v)| contains(&actual[k], v)),
        Value::Array(a) => actual
            .as_array()
            .is_some_and(|b| a.len() == b.len() && a.iter().zip(b).all(|(e, a)| contains(a, e))),
        _ => actual == expected,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identities_need_different_effective_passwords() {
        assert!(!distinct_passwords("same\n", "same\r\n"));
        assert!(!distinct_passwords("\n", "publisher"));
        assert!(distinct_passwords("reader\n", "publisher\n"));
    }
    #[test]
    fn omitted_network_rules_preserve_deny_without_ignoring_allow_rules() {
        let expected = json!({"policyTypes":["Egress"],"egress":[]});
        let mut actual = json!({"policyTypes":["Egress"]});
        normalize_empty_network_rules(&mut actual, &expected);
        assert!(contains(&actual, &expected));
        actual["egress"] = json!([{}]);
        normalize_empty_network_rules(&mut actual, &expected);
        assert!(!contains(&actual, &expected));
        actual["egress"] = json!([]);
        actual["policyTypes"] = json!(["Ingress"]);
        assert!(!contains(&actual, &expected));
    }
    #[test]
    fn only_standard_api_token_injection_can_be_adopted() {
        let expected = json!({"hostNetwork":false,"automountServiceAccountToken":true,"volumes":[],"containers":[{"name":"operator","volumeMounts":[]}]});
        let actual = json!({"automountServiceAccountToken":true,"volumes":[{"name":"kube-api-access-abcde","projected":serviceaccount_projection()}],"containers":[{"name":"operator","volumeMounts":[{"name":"kube-api-access-abcde","mountPath":"/var/run/secrets/kubernetes.io/serviceaccount","readOnly":true}]}]});
        let mut valid = actual.clone();
        normalize_serviceaccount_projection(&mut valid, &expected).unwrap();
        assert!(contains(&valid, &expected));
        let mut host_network = valid.clone();
        host_network["hostNetwork"] = json!(true);
        assert!(!contains(&host_network, &expected));
        let mut malicious = actual;
        malicious["volumes"][0]["projected"]["sources"][0]["serviceAccountToken"]["audience"] =
            json!("other-service");
        assert!(normalize_serviceaccount_projection(&mut malicious, &expected).is_err());
    }
    #[test]
    fn ownership_comparison_tolerates_defaults_but_not_added_containers() {
        assert!(contains(
            &json!({"spec":{"dnsPolicy":"ClusterFirst","containers":[{"name":"a","image":"digest","terminationMessagePath":"/dev/termination-log"}]}}),
            &json!({"spec":{"containers":[{"name":"a","image":"digest"}]}})
        ));
        assert!(!contains(
            &json!({"containers":[{"name":"a"},{"name":"b"}]}),
            &json!({"containers":[{"name":"a"}]})
        ));
    }
}

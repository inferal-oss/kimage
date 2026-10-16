//! Ordinary Kubernetes resources for registry-pulled installation images.
//! These JSON templates are also packaged into the Helm chart.
pub mod apply;
pub mod credentials;
use crate::{
    component_images::ImageCatalog,
    install::{InstallScope, RuntimeProfile},
};
use anyhow::{Context, Result, ensure};
use credentials::Credentials;
use k8s_openapi::api::core::v1::Node;
use kimage_core::{oci::Platform, scope::NamespaceScope};
use kube::ResourceExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Options {
    pub namespace: String,
    pub installation: String,
    pub scope: InstallScope,
    pub application_namespace: Option<String>,
    pub excluded_namespaces: BTreeSet<String>,
    pub runtime_profile: RuntimeProfile,
    pub registry_node: Option<String>,
    #[serde(default)]
    pub worker_nodes: BTreeMap<String, String>,
    #[serde(default)]
    pub export_origins: BTreeSet<String>,
    pub storage_class: Option<String>,
    pub storage_size: String,
    pub existing_claim: Option<String>,
    pub port: u16,
}
impl Options {
    pub fn scope(&self) -> NamespaceScope {
        NamespaceScope {
            system_namespace: self.namespace.clone(),
            application_namespace: self.application_namespace.clone(),
            cluster_wide: self.scope == InstallScope::Cluster,
            excluded_namespaces: self.excluded_namespaces.clone(),
        }
    }
    pub fn validate(&self) -> Result<()> {
        self.scope().validate()?;
        ensure!(
            kimage_core::selector::dns_label(&self.installation)
                && self.installation.len() <= 32
                && self.port >= 1024,
            "InvalidInstallationIdentity"
        );
        ensure!(
            self.storage_size.len() <= 32 && !self.storage_size.is_empty(),
            "InvalidStorageSize"
        );
        ensure!(
            self.export_origins.len() <= 8,
            "TooManyApprovedExportOrigins"
        );
        for value in &self.export_origins {
            let url: reqwest::Url = value.parse()?;
            ensure!(
                url.scheme() == "https"
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.path() == "/"
                    && url.query().is_none()
                    && url.fragment().is_none(),
                "InvalidApprovedExportOrigin"
            );
        }
        if let Some(claim) = &self.existing_claim {
            ensure!(
                kimage_core::selector::dns_subdomain(claim),
                "InvalidExistingClaim"
            );
        }
        if let Some(node) = &self.registry_node {
            ensure!(
                kimage_core::selector::dns_subdomain(node),
                "InvalidRegistryNode"
            );
        }
        Ok(())
    }
}

pub const TEMPLATE_NAMES: [&str; 12] = [
    "node-policy.json",
    "exporter.json",
    "base.json",
    "storage.json",
    "control.json",
    "scope-namespace.json",
    "scope-cluster.json",
    "proxy.json",
    "builder-pod.json",
    "builder.json",
    "builder-check.json",
    "webhooks.json",
];
pub fn template(name: &str) -> Result<&'static str> {
    Ok(match name {
        "node-policy.json" => include_str!("templates/node-policy.json"),
        "exporter.json" => include_str!("templates/exporter.json"),
        "base.json" => include_str!("templates/base.json"),
        "storage.json" => include_str!("templates/storage.json"),
        "control.json" => include_str!("templates/control.json"),
        "scope-namespace.json" => include_str!("templates/scope-namespace.json"),
        "scope-cluster.json" => include_str!("templates/scope-cluster.json"),
        "proxy.json" => include_str!("templates/proxy.json"),
        "builder-pod.json" => include_str!("templates/builder-pod.json"),
        "builder.json" => include_str!("templates/builder.json"),
        "builder-check.json" => include_str!("templates/builder-check.json"),
        "webhooks.json" => include_str!("templates/webhooks.json"),
        _ => anyhow::bail!("UnknownInstallTemplate"),
    })
}

// Published predecessor templates are part of the rollback contract. Rendering
// an old binary against current RBAC or recovery templates is not a rollback.
fn selected_template(name: &str, bindings: &BTreeMap<String, Value>) -> Result<&'static str> {
    if bindings.get("release").and_then(Value::as_str) == Some("0.4.1") {
        return Ok(match name {
            "base.json" => include_str!("templates/v0.4.1/base.json"),
            "builder-check.json" => include_str!("templates/v0.4.1/builder-check.json"),
            "builder-pod.json" => include_str!("templates/v0.4.1/builder-pod.json"),
            "builder.json" => include_str!("templates/v0.4.1/builder.json"),
            "control.json" => include_str!("templates/v0.4.1/control.json"),
            "exporter.json" => include_str!("templates/v0.4.1/exporter.json"),
            "proxy.json" => include_str!("templates/v0.4.1/proxy.json"),
            "scope-cluster.json" => include_str!("templates/v0.4.1/scope-cluster.json"),
            "scope-namespace.json" => include_str!("templates/v0.4.1/scope-namespace.json"),
            "storage.json" => include_str!("templates/v0.4.1/storage.json"),
            "webhooks.json" => include_str!("templates/v0.4.1/webhooks.json"),
            _ => anyhow::bail!("UnknownPredecessorTemplate"),
        });
    }
    template(name)
}

pub fn render(value: &Value, bindings: &BTreeMap<String, Value>) -> Result<Value> {
    Ok(match value {
        Value::String(source) => {
            if let Some(key) = source.strip_prefix("${").and_then(|s| s.strip_suffix('}'))
                && !key.contains("${")
                && !key.contains('}')
            {
                return bindings.get(key).cloned().context("MissingInstallBinding");
            }
            let mut rendered = source.clone();
            while let Some(start) = rendered.find("${") {
                let end = rendered[start..]
                    .find('}')
                    .context("InvalidInstallPlaceholder")?
                    + start;
                let key = &rendered[start + 2..end];
                let binding = bindings.get(key).context("MissingInstallBinding")?;
                let scalar = match binding {
                    Value::String(s) => s.clone(),
                    Value::Number(_) | Value::Bool(_) => binding.to_string(),
                    _ => anyhow::bail!("NonScalarInstallInterpolation"),
                };
                ensure!(!scalar.contains("${"), "RecursiveInstallInterpolation");
                rendered.replace_range(start..=end, &scalar);
            }
            Value::String(rendered)
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|v| render(v, bindings))
                .collect::<Result<_>>()?,
        ),
        Value::Object(values) => {
            let mut result = serde_json::Map::new();
            for (key, value) in values {
                let value = render(value, bindings)?;
                if !value.is_null() {
                    result.insert(key.clone(), value);
                }
            }
            Value::Object(result)
        }
        value => value.clone(),
    })
}

pub struct Plan {
    pub resources: Vec<Value>,
    pub webhooks: Vec<Value>,
    pub primary_node: String,
    pub nodes: Vec<Node>,
    pub workers: BTreeMap<String, String>,
}
fn append(
    resources: &mut Vec<Value>,
    name: &str,
    bindings: &BTreeMap<String, Value>,
) -> Result<()> {
    let value = render(
        &serde_json::from_str(selected_template(name, bindings)?)?,
        bindings,
    )?;
    resources.extend(
        value
            .as_array()
            .context("InstallResourceArrayRequired")?
            .iter()
            .cloned(),
    );
    Ok(())
}

pub fn eligible_nodes(nodes: &[Node], profile: RuntimeProfile) -> Result<Vec<Node>> {
    let mut eligible = Vec::new();
    for node in nodes {
        if node.metadata.deletion_timestamp.is_some()
            || node
                .spec
                .as_ref()
                .is_some_and(|s| s.unschedulable == Some(true))
            || !node
                .status
                .as_ref()
                .and_then(|s| s.conditions.as_ref())
                .is_some_and(|cs| cs.iter().any(|c| c.type_ == "Ready" && c.status == "True"))
        {
            continue;
        }
        let info = node
            .status
            .as_ref()
            .and_then(|s| s.node_info.as_ref())
            .context("MissingNodeInfo")?;
        let platform = Platform {
            os: info.operating_system.clone(),
            architecture: info.architecture.clone(),
        };
        if !platform.supported() {
            continue;
        }
        ensure!(
            node.labels().get("kubernetes.io/os") == Some(&platform.os)
                && node.labels().get("kubernetes.io/arch") == Some(&platform.architecture),
            "NodePlatformLabelsMismatch"
        );
        ensure!(
            match profile {
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
            node.uid()
                .is_some_and(|uid| !uid.is_empty() && uid.len() <= 128),
            "MissingNodeUid"
        );
        eligible.push(node.clone());
    }
    eligible.sort_by_key(ResourceExt::name_any);
    ensure!(
        !eligible.is_empty() && eligible.len() <= 128,
        "NoSupportedReadyNodesOrTooManyNodes"
    );
    Ok(eligible)
}

impl Plan {
    pub fn new(
        options: &Options,
        catalog: &ImageCatalog,
        nodes: &[Node],
        credentials: &Credentials,
    ) -> Result<Self> {
        options.validate()?;
        catalog.validate(&catalog.release, false)?;
        credentials.validate()?;
        let nodes = eligible_nodes(nodes, options.runtime_profile)?;
        let primary = options
            .registry_node
            .as_deref()
            .unwrap_or_else(|| nodes[0].metadata.name.as_deref().unwrap());
        ensure!(
            nodes.iter().any(|node| node.name_any() == primary),
            "RegistryNodeNotEligible"
        );
        let primary = primary.to_owned();
        let mut workers = BTreeMap::new();
        for node in &nodes {
            workers
                .entry(
                    node.status
                        .as_ref()
                        .unwrap()
                        .node_info
                        .as_ref()
                        .unwrap()
                        .architecture
                        .clone(),
                )
                .or_insert_with(|| node.name_any());
        }
        for (architecture, name) in &options.worker_nodes {
            ensure!(
                nodes.iter().any(|node| node.name_any() == *name
                    && node
                        .status
                        .as_ref()
                        .and_then(|s| s.node_info.as_ref())
                        .is_some_and(|i| i.architecture == *architecture)),
                "RetainedWorkerNodeUnavailable"
            );
            workers.insert(architecture.clone(), name.clone());
        }
        let scope = options.scope();
        let origin = format!(
            "https://{}-registry.{}.svc:5443/",
            options.installation, options.namespace
        );
        let allowed = scope
            .registry_namespaces()
            .into_iter()
            .collect::<Vec<_>>()
            .join(",");
        let mut b = BTreeMap::<String, Value>::new();
        let mut persisted_options = serde_json::to_value(options)?;
        persisted_options["registryNode"] = json!(primary);
        persisted_options["workerNodes"] = json!(workers);
        let registry_uid = nodes
            .iter()
            .find(|node| node.name_any() == primary)
            .and_then(ResourceExt::uid)
            .context("MissingRegistryNodeUid")?;
        b.insert("release".into(), json!(catalog.release));
        b.insert("manager".into(), json!("cli"));
        for (key, value) in [
            ("namespace", json!(options.namespace)),
            ("installation", json!(options.installation)),
            ("applicationNamespace", json!(options.application_namespace)),
            ("primaryNode", json!(primary)),
            ("port", json!(options.port)),
            ("registryOrigin", json!(origin)),
            ("allowedNamespaces", json!(allowed)),
            ("scopeSelector", scope.webhook_selector()),
            ("scopeJson", json!(serde_json::to_string(&scope)?)),
            ("storageClass", json!(options.storage_class)),
            ("storageSize", json!(options.storage_size)),
            (
                "claimName",
                json!(
                    options
                        .existing_claim
                        .clone()
                        .unwrap_or_else(|| format!("{}-data", options.installation))
                ),
            ),
            (
                "registryConfiguration",
                json!(crate::install::REGISTRY_CONFIG),
            ),
            (
                "buildConfiguration",
                json!("{\"schema\":1,\"buildPolicy\":\"OnDemand\"}"),
            ),
            ("imageCatalog", json!(serde_json::to_string(catalog)?)),
            (
                "installationConfiguration",
                json!(serde_json::to_string(
                    &json!({"schema":1,"delivery":"ImagePull","options":persisted_options,"registryNodeUid":registry_uid,"release":catalog.release})
                )?),
            ),
            ("registryCredentials", json!(credentials.registry)),
            ("admissionCredentials", json!(credentials.admission)),
            ("builderCredentials", json!(credentials.builder)),
            (
                "proxyCredentials",
                json!(Credentials::subset(
                    &credentials.registry,
                    &["ca.crt", "reader-password"]
                )?),
            ),
            (
                "publisherCredentials",
                json!(Credentials::subset(
                    &credentials.registry,
                    &["ca.crt", "publisher-password"]
                )?),
            ),
            (
                "builderServerCredentials",
                json!(Credentials::subset(
                    &credentials.builder,
                    &["ca.crt", "server.crt", "server.key"]
                )?),
            ),
            (
                "builderClientCredentials",
                json!(Credentials::subset(
                    &credentials.builder,
                    &["ca.crt", "client.crt", "client.key"]
                )?),
            ),
            ("admissionCa", json!(credentials.admission["ca.crt"])),
            (
                "workerCapabilities",
                json!(crate::builder_install::WORKER_CAPABILITIES),
            ),
        ] {
            b.insert(key.into(), value);
        }
        for role in crate::component_images::ROLES {
            b.insert(format!("{role}Image"), json!(catalog.reference(role)?));
        }
        let input = catalog.reference("input")?;
        let builder = catalog.reference("builder")?;
        let mut op = vec![
            "--system-namespace".into(),
            options.namespace.clone(),
            "--node".into(),
            primary.clone(),
            "--input-image".into(),
            input.clone(),
            "--registry-origin".into(),
            origin.clone(),
            "--registry-ca".into(),
            "/credentials/ca.crt".into(),
            "--publisher-password".into(),
            "/credentials/publisher-password".into(),
            "--installation".into(),
            options.installation.clone(),
            "--builder-image".into(),
            builder.clone(),
        ];
        let mut admission = vec![
            "--system-namespace".into(),
            options.namespace.clone(),
            "--installation".into(),
            options.installation.clone(),
            "--endpoint".into(),
            format!("127.0.0.1:{}", options.port),
            "--nodes".into(),
            primary.clone(),
            "--cert".into(),
            "/credentials/tls.crt".into(),
            "--key".into(),
            "/credentials/tls.key".into(),
            "--resolution-key".into(),
            "/credentials/resolution-key".into(),
            "--operator-identity".into(),
            format!(
                "system:serviceaccount:{}:{}-operator",
                options.namespace, options.installation
            ),
            "--input-image".into(),
            input.clone(),
            "--builder-image".into(),
            builder.clone(),
        ];
        for args in [&mut op, &mut admission] {
            if scope.cluster_wide {
                args.push("--cluster-wide".into());
                for ns in &options.excluded_namespaces {
                    args.extend(["--exclude-namespace".into(), ns.clone()]);
                }
            } else {
                args.extend([
                    "--namespace".into(),
                    options
                        .application_namespace
                        .clone()
                        .context("ApplicationNamespaceRequired")?,
                ]);
            }
        }
        b.insert("operatorArguments".into(), json!(op));
        b.insert("admissionArguments".into(), json!(admission));
        let mut resources = Vec::new();
        if options.existing_claim.is_none() {
            append(&mut resources, "storage.json", &b)?;
        }
        append(&mut resources, "base.json", &b)?;
        if catalog.release == "0.5.0" {
            b.insert(
                "nodePolicy".into(),
                json!(serde_json::to_string(
                    &crate::node_reconcile::Policy::default()
                )?),
            );
            append(&mut resources, "node-policy.json", &b)?;
        }
        append(
            &mut resources,
            if scope.cluster_wide {
                "scope-cluster.json"
            } else {
                "scope-namespace.json"
            },
            &b,
        )?;
        append(&mut resources, "control.json", &b)?;
        if !options.export_origins.is_empty() {
            let mut args = vec![
                "--system-namespace".to_owned(),
                options.namespace.clone(),
                "--installation".into(),
                options.installation.clone(),
                "--internal-origin".into(),
                origin.clone(),
                "--internal-ca".into(),
                "/credentials/ca.crt".into(),
                "--internal-password".into(),
                "/credentials/reader-password".into(),
            ];
            if scope.cluster_wide {
                args.push("--cluster-wide".into());
                for ns in &options.excluded_namespaces {
                    args.extend(["--exclude-namespace".into(), ns.clone()]);
                }
            } else {
                args.extend([
                    "--namespace".into(),
                    options.application_namespace.clone().unwrap(),
                ]);
            }
            for origin in &options.export_origins {
                args.extend(["--approved-origin".into(), origin.clone()]);
            }
            b.insert("exporterArguments".into(), json!(args));
            b.insert(
                "exporterRoleKind".into(),
                json!(if scope.cluster_wide {
                    "ClusterRole"
                } else {
                    "Role"
                }),
            );
            b.insert(
                "exporterBindingKind".into(),
                json!(if scope.cluster_wide {
                    "ClusterRoleBinding"
                } else {
                    "RoleBinding"
                }),
            );
            b.insert(
                "exporterApplicationNamespace".into(),
                json!(options.application_namespace),
            );
            b.insert(
                "exporterApplicationName".into(),
                json!(format!(
                    "{}-{}",
                    options.installation,
                    if scope.cluster_wide {
                        "application-exporter"
                    } else {
                        "exporter"
                    }
                )),
            );
            append(&mut resources, "exporter.json", &b)?;
        }

        append_native(&mut resources, &mut b, options, catalog, &nodes, &workers)?;
        let mut webhooks = Vec::new();
        append(&mut webhooks, "webhooks.json", &b)?;
        Ok(Self {
            resources,
            webhooks,
            primary_node: primary,
            nodes,
            workers,
        })
    }
}

fn append_native(
    resources: &mut Vec<Value>,
    b: &mut BTreeMap<String, Value>,
    options: &Options,
    catalog: &ImageCatalog,
    nodes: &[Node],
    workers: &BTreeMap<String, String>,
) -> Result<()> {
    let scope = options.scope();
    let origin = format!(
        "https://{}-registry.{}.svc:5443/",
        options.installation, options.namespace
    );
    let builder = catalog.reference("builder")?;
    let input = catalog.reference("input")?;
    for node in nodes {
        let uid = node.uid().context("MissingNodeUid")?;
        b.insert("node".into(), json!(node.name_any()));
        b.insert("nodeUid".into(), json!(uid));
        b.insert(
            "nodeScope".into(),
            json!(crate::node_install::scope(&options.installation, &uid)),
        );
        let endpoint = crate::node_install::Endpoint {
            schema: 1,
            node: node.name_any(),
            node_uid: uid,
            port: options.port,
            origin: origin.clone(),
            namespaces: scope.registry_namespaces(),
            proxy_images: BTreeSet::from([catalog.reference("proxy")?]),
        };
        b.insert(
            "nodeEndpoint".into(),
            json!(serde_json::to_string(&endpoint)?),
        );
        append(resources, "proxy.json", b)?;
    }
    for (arch, node) in workers {
        b.insert(
            "nodeUid".into(),
            json!(
                nodes
                    .iter()
                    .find(|n| n.name_any() == *node)
                    .and_then(ResourceExt::uid)
                    .context("MissingWorkerNodeUid")?
            ),
        );
        b.insert("architecture".into(), json!(arch));
        b.insert("node".into(), json!(node));
        b.insert(
            "workerConfiguration".into(),
            json!(crate::builder_install::configuration(arch)),
        );
        let pod = render(
            &serde_json::from_str(selected_template("builder-pod.json", b)?)?,
            b,
        )?;
        b.insert("builderPodJson".into(), json!(serde_json::to_string(&pod)?));
        let worker = json!({"schema":2,"builders":{"default":{"engine":"BuildKit","image":builder,"inputImage":input,"version":"0.33.0","frontendVersion":"1.27.0","endpoint":format!("tcp://{}-builder-{arch}.{}.svc:1234",options.installation,options.namespace),"tlsServerName":format!("{}-builder.{}.svc",options.installation,options.namespace),"clientSecret":format!("{}-builder-client-{arch}",options.installation),"formats":["Containerfile"],"strategies":["Direct"],"platforms":[format!("linux/{arch}")],"node":node,"isolation":"PodUserNamespace","trustScope":options.application_namespace.as_deref().unwrap_or("*"),"cache":{"modes":["Local","Disabled"],"maxUsedBytes":2147483648_u64,"volumeLimitBytes":4294967296_u64},"maxParallelism":1,"offline":true}}});
        b.insert(
            "builderCatalog".into(),
            json!(serde_json::to_string(&worker)?),
        );
        append(resources, "builder.json", b)?;
        resources.push(pod);
        b.insert(
            "workerCheckName".into(),
            json!(format!(
                "{}-builder-check-{arch}-{}",
                options.installation,
                if catalog.release == "0.5.0" {
                    kimage_core::oci::digest(
                        format!(
                            "{node}:{}:{}:{}",
                            b["nodeUid"].as_str().unwrap(),
                            catalog.components["builder"].digest,
                            catalog.components["input"].digest
                        )
                        .as_bytes(),
                    )[7..19]
                        .to_owned()
                } else {
                    catalog.components["builder"].digest[7..15].to_owned()
                }
            )),
        );
        append(resources, "builder-check.json", b)?;
    }
    Ok(())
}
/// Minimal credentials and bindings for node reconciliation. The operator never
/// needs registry publisher passwords or the admission signing key.
pub fn native_resources(
    options: &Options,
    catalog: &ImageCatalog,
    nodes: &[Node],
    workers: &BTreeMap<String, String>,
    builder_credentials: &credentials::Material,
    manager: &str,
) -> Result<Vec<Value>> {
    let scope = options.scope();
    let mut b = BTreeMap::from([
        ("release".into(), json!(catalog.release)),
        ("manager".into(), json!(manager)),
        ("installation".into(), json!(options.installation)),
        ("namespace".into(), json!(options.namespace)),
        (
            "applicationNamespace".into(),
            json!(options.application_namespace),
        ),
        ("port".into(), json!(options.port)),
        (
            "registryOrigin".into(),
            json!(format!(
                "https://{}-registry.{}.svc:5443/",
                options.installation, options.namespace
            )),
        ),
        (
            "allowedNamespaces".into(),
            json!(
                scope
                    .registry_namespaces()
                    .into_iter()
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        ),
        ("scopeSelector".into(), scope.webhook_selector()),
        ("proxyImage".into(), json!(catalog.reference("proxy")?)),
        ("builderImage".into(), json!(catalog.reference("builder")?)),
        ("inputImage".into(), json!(catalog.reference("input")?)),
        (
            "builderServerCredentials".into(),
            json!(Credentials::subset(
                builder_credentials,
                &["ca.crt", "server.crt", "server.key"]
            )?),
        ),
        (
            "builderClientCredentials".into(),
            json!(Credentials::subset(
                builder_credentials,
                &["ca.crt", "client.crt", "client.key"]
            )?),
        ),
        (
            "workerCapabilities".into(),
            json!(crate::builder_install::WORKER_CAPABILITIES),
        ),
    ]);
    let mut resources = Vec::new();
    append_native(&mut resources, &mut b, options, catalog, nodes, workers)?;
    Ok(resources)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn node(name: &str, arch: &str) -> Node {
        serde_json::from_value(json!({"apiVersion":"v1","kind":"Node","metadata":{"name":name,"uid":format!("uid-{name}"),"labels":{"kubernetes.io/os":"linux","kubernetes.io/arch":arch}},"spec":{},"status":{"conditions":[{"type":"Ready","status":"True"}],"nodeInfo":{"architecture":arch,"operatingSystem":"linux","kubeletVersion":"v1.36.5+rke2r1","containerRuntimeVersion":"containerd://2.3.4-k3s1","bootID":"boot","kernelVersion":"test","machineID":"machine","osImage":"fixture","systemUUID":"system"}}})).unwrap()
    }
    fn catalog() -> ImageCatalog {
        use crate::component_images::{Component, OFFICIAL_REPOSITORY, ROLES};
        ImageCatalog {
            schema: 1,
            release: "0.4.0".into(),
            api_schema_revision: crate::schema::REVISION,
            repository: OFFICIAL_REPOSITORY.into(),
            components: ROLES
                .into_iter()
                .map(|role| {
                    (
                        role.into(),
                        Component {
                            version: match role {
                                "gateway" | "proxy" => crate::serving::VERSION,
                                "registry" => "3.1.2",
                                "builder" => "0.33.0",
                                _ => "0.4.0",
                            }
                            .into(),
                            digest: format!("sha256:{}", "a".repeat(64)),
                            platforms: BTreeSet::from([
                                Platform::linux_amd64(),
                                Platform {
                                    os: "linux".into(),
                                    architecture: "arm64".into(),
                                },
                            ]),
                        },
                    )
                })
                .collect(),
        }
    }
    fn options(scope: InstallScope) -> Options {
        Options {
            namespace: "kimage-system".into(),
            installation: "kimage".into(),
            scope,
            application_namespace: (scope == InstallScope::Namespace).then(|| "apps".into()),
            excluded_namespaces: if scope == InstallScope::Cluster {
                BTreeSet::from(["excluded".into()])
            } else {
                BTreeSet::new()
            },
            runtime_profile: RuntimeProfile::Inferal,
            registry_node: None,
            worker_nodes: BTreeMap::new(),
            export_origins: BTreeSet::new(),
            storage_class: None,
            storage_size: "1Gi".into(),
            existing_claim: None,
            port: 17701,
        }
    }
    #[test]
    fn both_scopes_use_native_registry_pulls_and_retained_storage_without_binary_payloads() {
        let credentials = Credentials::generate("kimage-system", "kimage").unwrap();
        for scope in [InstallScope::Namespace, InstallScope::Cluster] {
            let opts = options(scope);
            let plan = Plan::new(
                &opts,
                &catalog(),
                &[node("amd", "amd64"), node("arm", "arm64")],
                &credentials,
            )
            .unwrap();
            assert_eq!(
                plan.workers,
                BTreeMap::from([
                    ("amd64".into(), "amd".into()),
                    ("arm64".into(), "arm".into())
                ])
            );
            let bytes = serde_json::to_vec(&plan.resources).unwrap();
            assert!(
                bytes.len() < 128 * 1024,
                "installation resources must remain small"
            );
            assert!(!String::from_utf8(bytes).unwrap().contains("binaryData"));
            let accounts = plan
                .resources
                .iter()
                .filter(|v| v["kind"] == "ServiceAccount")
                .map(|v| v["metadata"]["name"].as_str().unwrap())
                .collect::<BTreeSet<_>>();
            let mut proxies = 0;
            for resource in &plan.resources {
                if resource["kind"] == "ConfigMap" {
                    assert!(
                        !resource["metadata"]["name"]
                            .as_str()
                            .unwrap()
                            .contains("launcher")
                    );
                }
                let spec = if resource["kind"] == "Deployment" || resource["kind"] == "Job" {
                    Some(&resource["spec"]["template"]["spec"])
                } else if resource["kind"] == "Pod" {
                    Some(&resource["spec"])
                } else {
                    None
                };
                if let Some(spec) = spec {
                    if spec["automountServiceAccountToken"] == false {
                        assert!(
                            accounts.contains(spec["serviceAccountName"].as_str().unwrap()),
                            "tokenless workloads must reference a synchronously created installation account"
                        );
                    }
                    for container in spec["containers"].as_array().unwrap() {
                        assert!(
                            container["image"]
                                .as_str()
                                .unwrap()
                                .starts_with("ghcr.io/inferal-oss/kimage/")
                        );
                        assert!(container["image"].as_str().unwrap().contains("@sha256:"));
                        assert_eq!(container["imagePullPolicy"], "IfNotPresent");
                    }
                    assert!(
                        spec["volumes"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .all(|v| v.get("hostPath").is_none())
                    );
                    if resource["metadata"]["labels"]["kimage.dev/role"] == "node-proxy" {
                        proxies += 1;
                        assert_eq!(spec["hostNetwork"], true);
                    }
                    if resource["metadata"]["labels"]["kimage.dev/role"] == "builder" {
                        assert_eq!(spec["hostUsers"], false);
                    }
                }
            }
            assert_eq!(proxies, 2);
            let pvc = plan
                .resources
                .iter()
                .find(|v| v["kind"] == "PersistentVolumeClaim")
                .unwrap();
            assert_eq!(
                pvc["metadata"]["annotations"]["helm.sh/resource-policy"],
                "keep"
            );
            assert!(pvc["spec"].get("storageClassName").is_none());
            let app_role = plan
                .resources
                .iter()
                .find(|v| v["metadata"]["name"] == "kimage-application-operator")
                .unwrap();
            if scope == InstallScope::Namespace {
                assert_eq!(app_role["kind"], "Role");
                assert_eq!(app_role["metadata"]["namespace"], "apps");
            } else {
                assert_eq!(app_role["kind"], "ClusterRole");
                assert!(app_role["metadata"].get("namespace").is_none());
            }
            assert_eq!(plan.webhooks.len(), 2);
        }
    }
    #[test]
    fn placement_requires_real_native_ready_nodes_and_honors_an_existing_claim() {
        let credentials = Credentials::generate("kimage-system", "kimage").unwrap();
        let mut opts = options(InstallScope::Namespace);
        opts.existing_claim = Some("retained-data".into());
        opts.registry_node = Some("arm".into());
        let plan = Plan::new(&opts, &catalog(), &[node("arm", "arm64")], &credentials).unwrap();
        assert_eq!(plan.primary_node, "arm");
        assert_eq!(plan.workers.keys().collect::<Vec<_>>(), vec!["arm64"]);
        assert!(
            !plan
                .resources
                .iter()
                .any(|v| v["kind"] == "PersistentVolumeClaim")
        );
        let mut wrong = node("arm", "arm64");
        wrong
            .metadata
            .labels
            .as_mut()
            .unwrap()
            .insert("kubernetes.io/arch".into(), "amd64".into());
        assert!(Plan::new(&opts, &catalog(), &[wrong], &credentials).is_err());
        assert!(Plan::new(&opts, &catalog(), &[], &credentials).is_err());
    }
    #[test]
    fn typed_bindings_preserve_values_and_refuse_missing_or_recursive_tokens() {
        let b = BTreeMap::from([
            ("port".into(), json!(17701)),
            ("text".into(), json!("quoted \"value\"\n")),
            ("none".into(), Value::Null),
        ]);
        let v=render(&json!({"port":"${port}","text":"${text}","url":"127.0.0.1:${port}","absent":"${none}","empty":[]}),&b).unwrap();
        assert_eq!(
            v,
            json!({"port":17701,"text":"quoted \"value\"\n","url":"127.0.0.1:17701","empty":[]})
        );
        assert!(render(&json!("${missing}"), &b).is_err());
        assert!(render(&json!("prefix${none}"), &b).is_err());
    }
}

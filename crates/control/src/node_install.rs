//! Administrator-approved node endpoint projections and API-only proxy recovery.
use crate::{
    bundle::Bundle,
    install::{Installer, LABEL},
};
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use k8s_openapi::api::{
    core::v1::{ConfigMap, Node, Pod},
    rbac::v1::{ClusterRole, Role},
};
use kube::{
    Api, ResourceExt,
    api::{DeleteParams, ListParams, PostParams, Preconditions},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeSet;
use std::time::Duration;

pub const ENDPOINT_ROLE: &str = "node-endpoint";
pub fn scope(installation: &str, node_uid: &str) -> String {
    format!(
        "{installation}-n-{}",
        &kimage_core::oci::digest(node_uid.as_bytes())[7..23]
    )
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Endpoint {
    pub schema: u32,
    pub node: String,
    pub node_uid: String,
    pub port: u16,
    pub origin: String,
    pub namespaces: BTreeSet<String>,
    pub proxy_images: BTreeSet<String>,
}
impl Endpoint {
    pub fn validate(
        &self,
        installation: &str,
        system: &str,
        application: &str,
        port: u16,
    ) -> Result<()> {
        ensure!(
            self.schema == 1
                && self.port == port
                && port >= 1024
                && kimage_core::selector::dns_subdomain(&self.node)
                && !self.node_uid.is_empty()
                && self.node_uid.len() <= 128,
            "InvalidNodeEndpoint"
        );
        ensure!(
            self.origin == format!("https://{installation}-registry.{system}.svc:5443/")
                && self.namespaces == BTreeSet::from([system.to_owned(), application.to_owned()]),
            "NodeEndpointScopeMismatch"
        );
        ensure!(
            !self.proxy_images.is_empty() && self.proxy_images.len() <= 2,
            "InvalidNodeProxyImages"
        );
        let prefix = format!("127.0.0.1:{port}/{system}/system-proxy@");
        ensure!(
            self.proxy_images.iter().all(|r| r
                .strip_prefix(&prefix)
                .is_some_and(kimage_core::oci::valid_digest)),
            "InvalidNodeProxyImage"
        );
        Ok(())
    }
}
pub fn ready(pod: &Pod) -> bool {
    pod.metadata.deletion_timestamp.is_none()
        && pod
            .status
            .as_ref()
            .and_then(|s| s.conditions.as_ref())
            .is_some_and(|c| c.iter().any(|c| c.type_ == "Ready" && c.status == "True"))
}
pub fn proxy_matches(pod: &Pod, endpoint: &Endpoint, installation: &str) -> bool {
    let expected = scope(installation, &endpoint.node_uid);
    let spec = serde_json::to_value(&pod.spec).unwrap_or_default();
    let profile = json!({"dnsPolicy":"ClusterFirstWithHostNet","securityContext":{"seccompProfile":{"type":"RuntimeDefault"}},"containers":[{"name":"proxy","imagePullPolicy":"IfNotPresent","securityContext":{"allowPrivilegeEscalation":false,"readOnlyRootFilesystem":true,"capabilities":{"drop":["ALL"]}},"readinessProbe":{"httpGet":{"host":"127.0.0.1","path":"/v2/","port":endpoint.port}},"volumeMounts":[{"name":"credentials","mountPath":"/credentials","readOnly":true}]}],"volumes":[{"name":"credentials","secret":{"secretName":format!("{installation}-proxy-credentials")}}]});
    if !crate::reconcile::matches_declared(&profile, &spec) {
        return false;
    }
    let Some(container) = pod.spec.as_ref().and_then(|s| s.containers.first()) else {
        return false;
    };
    if container.security_context.as_ref().is_none_or(|s| {
        s.privileged == Some(true)
            || s.capabilities
                .as_ref()
                .is_some_and(|c| c.add.as_ref().is_some_and(|a| !a.is_empty()))
    }) {
        return false;
    }
    let Some(args) = &container.args else {
        return false;
    };
    let argument = |flag: &str| {
        args.windows(2)
            .find(|v| v[0] == flag)
            .map(|v| v[1].as_str())
    };
    if argument("--listen") != Some(format!("127.0.0.1:{}", endpoint.port).as_str())
        || argument("--upstream") != Some(endpoint.origin.as_str())
        || argument("--ca") != Some("/credentials/ca.crt")
        || argument("--password-file") != Some("/credentials/reader-password")
        || !argument("--namespaces").is_some_and(|s| {
            s.split(',').map(str::to_owned).collect::<BTreeSet<_>>() == endpoint.namespaces
        })
    {
        return false;
    }
    pod.name_any() == format!("{expected}-proxy")
        && pod.labels().get(LABEL).is_some_and(|v| v == installation)
        && pod.annotations().get("kimage.dev/node-uid") == Some(&endpoint.node_uid)
        && pod.spec.as_ref().is_some_and(|s| {
            s.node_name.as_ref() == Some(&endpoint.node)
                && s.host_network == Some(true)
                && s.host_pid != Some(true)
                && s.host_ipc != Some(true)
                && s.automount_service_account_token == Some(false)
                && s.containers.len() == 1
                && s.containers[0]
                    .image
                    .as_ref()
                    .is_some_and(|i| endpoint.proxy_images.contains(i))
                && s.containers[0]
                    .command
                    .as_ref()
                    .is_some_and(|c| c == &["/kimage-proxy"])
                && !s.volumes.iter().flatten().any(|v| v.host_path.is_some())
        })
}

impl Installer {
    /// Only the exact shipped single-node read rules may expand to projections.
    pub(crate) async fn readiness_permissions(&self, additional_node: Option<&str>) -> Result<()> {
        let name = self.name("admission");
        let role_api: Api<Role> = Api::namespaced(self.client.clone(), &self.options.namespace);
        let old_rules = json!([{"apiGroups":[""],"resources":["pods"],"verbs":["get"],"resourceNames":[self.name("proxy"),self.name("proxy-next")]}]);
        let new_rules =
            json!([{"apiGroups":[""],"resources":["pods","configmaps"],"verbs":["get","list"]}]);
        let mut desired: Role = serde_json::from_value(
            json!({"apiVersion":"rbac.authorization.k8s.io/v1","kind":"Role","metadata":self.metadata(&name),"rules":new_rules}),
        )?;
        if let Some(existing) = role_api.get_opt(&name).await? {
            ensure!(
                existing.labels().get(LABEL) == Some(&self.options.installation),
                "ReadinessRoleOwnershipConflict"
            );
            let rules = serde_json::to_value(&existing.rules)?;
            ensure!(
                rules == old_rules || rules == new_rules,
                "ReadinessRoleDefinitionConflict"
            );
            if rules != new_rules {
                desired.metadata = existing.metadata;
                role_api
                    .replace(&name, &PostParams::default(), &desired)
                    .await?;
            }
        } else {
            role_api.create(&PostParams::default(), &desired).await?;
        }
        let api: Api<ClusterRole> = Api::all(self.client.clone());
        let mut approved = BTreeSet::from([self.options.node.clone()]);
        if let Some(node) = additional_node {
            ensure!(
                kimage_core::selector::dns_subdomain(node),
                "InvalidNodeName"
            );
            approved.insert(node.into());
        }
        let existing = api.get_opt(&name).await?;
        if let Some(existing) = &existing {
            ensure!(
                existing.labels().get(LABEL) == Some(&self.options.installation),
                "ReadinessRoleOwnershipConflict"
            );
            let mut rules = serde_json::to_value(&existing.rules)?;
            ensure!(
                rules.as_array().is_some_and(|r| r.len() == 1),
                "ReadinessRoleDefinitionConflict"
            );
            let names = rules[0]
                .as_object_mut()
                .context("InvalidReadinessRule")?
                .remove("resourceNames")
                .context("UnboundedReadinessRole")?;
            let names = names.as_array().context("InvalidReadinessNodes")?;
            ensure!(
                !names.is_empty() && names.len() <= 128,
                "InvalidReadinessNodes"
            );
            for node in names {
                let node = node.as_str().context("InvalidNodeName")?;
                ensure!(
                    kimage_core::selector::dns_subdomain(node),
                    "InvalidNodeName"
                );
                approved.insert(node.to_owned());
            }
            ensure!(
                rules == json!([{"apiGroups":[""],"resources":["nodes"],"verbs":["get"]}]),
                "ReadinessRoleDefinitionConflict"
            );
        }
        ensure!(approved.len() <= 128, "TooManyReadinessNodes");
        let mut desired: ClusterRole = serde_json::from_value(
            json!({"apiVersion":"rbac.authorization.k8s.io/v1","kind":"ClusterRole","metadata":self.metadata(&name),"rules":[{"apiGroups":[""],"resources":["nodes"],"verbs":["get"],"resourceNames":approved}]}),
        )?;
        if let Some(existing) = existing {
            if existing.rules != desired.rules {
                desired.metadata = existing.metadata;
                api.replace(&name, &PostParams::default(), &desired).await?;
            }
        } else {
            api.create(&PostParams::default(), &desired).await?;
        }

        Ok(())
    }
    pub(crate) async fn approve_endpoint(&self, endpoint: &Endpoint) -> Result<()> {
        let app = self
            .options
            .application_namespace
            .as_deref()
            .context("ApplicationNamespaceRequired")?;
        endpoint.validate(
            &self.options.installation,
            &self.options.namespace,
            app,
            self.options.port,
        )?;
        let name = format!(
            "{}-endpoint",
            scope(&self.options.installation, &endpoint.node_uid)
        );
        let mut metadata = self.metadata(&name);
        metadata["labels"]["kimage.dev/role"] = json!(ENDPOINT_ROLE);
        let mut desired: ConfigMap = serde_json::from_value(
            json!({"apiVersion":"v1","kind":"ConfigMap","metadata":metadata,"data":{"endpoint.json":serde_json::to_string(endpoint)?}}),
        )?;
        let api: Api<ConfigMap> = Api::namespaced(self.client.clone(), &self.options.namespace);
        if let Some(existing) = api.get_opt(&name).await? {
            ensure!(
                existing.labels().get(LABEL) == Some(&self.options.installation)
                    && existing
                        .labels()
                        .get("kimage.dev/role")
                        .is_some_and(|s| s == ENDPOINT_ROLE),
                "NodeEndpointOwnershipConflict"
            );
            let old: Endpoint = serde_json::from_str(
                existing
                    .data
                    .as_ref()
                    .and_then(|d| d.get("endpoint.json"))
                    .context("MissingNodeEndpoint")?,
            )?;
            let mut identity = old.clone();
            identity.proxy_images = endpoint.proxy_images.clone();
            ensure!(identity == *endpoint, "NodeEndpointIdentityConflict");
            if old != *endpoint {
                desired.metadata = existing.metadata;
                api.replace(&name, &PostParams::default(), &desired).await?;
            }
        } else {
            api.create(&PostParams::default(), &desired).await?;
        }
        Ok(())
    }
    async fn delete_node_pod(&self, name: &str, uid: Option<String>) -> Result<()> {
        let api: Api<Pod> = Api::namespaced(self.client.clone(), &self.options.namespace);
        if let Some(pod) = api.get_opt(name).await? {
            ensure!(
                pod.labels().get(LABEL) == Some(&self.options.installation) && pod.uid() == uid,
                "NodeProxyOwnershipConflict"
            );
            api.delete(
                name,
                &DeleteParams {
                    grace_period_seconds: Some(1),
                    preconditions: Some(Preconditions {
                        uid: pod.uid(),
                        resource_version: pod.resource_version(),
                    }),
                    ..Default::default()
                },
            )
            .await?;
            self.wait_absent(name).await?;
        }
        Ok(())
    }
    async fn cleanup_node_payload(&self, bundle: &Bundle, prefix: &str) -> Result<()> {
        let (chunks, _) = bundle.chunks("proxy")?;
        let mut expected = vec![(
            format!("{prefix}-launcher"),
            json!({"launcher":STANDARD.encode(std::fs::read(bundle.path(&bundle.inventory.launcher)?)?)}),
        )];
        expected.extend(chunks.iter().enumerate().map(|(i, b)| {
            (
                format!("{prefix}-proxy-{i:04}"),
                json!({format!("{i:04}"):STANDARD.encode(b)}),
            )
        }));
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.options.namespace);
        let page = pods.list(&ListParams::default().limit(512)).await?;
        ensure!(
            page.metadata
                .continue_
                .as_ref()
                .is_none_or(|v| v.is_empty()),
            "TooManyServingPods"
        );
        let pods = serde_json::to_string(&page.items)?;
        let maps: Api<ConfigMap> = Api::namespaced(self.client.clone(), &self.options.namespace);
        for (name, data) in expected {
            ensure!(
                !pods.contains(&format!("\"{name}\"")),
                "NodePayloadStillMounted"
            );
            if let Some(map) = maps.get_opt(&name).await? {
                ensure!(
                    map.labels().get(LABEL) == Some(&self.options.installation)
                        && map.immutable == Some(true)
                        && serde_json::to_value(&map.binary_data)? == data,
                    "NodePayloadCleanupConflict"
                );
                maps.delete(
                    &name,
                    &DeleteParams {
                        preconditions: Some(Preconditions {
                            uid: map.uid(),
                            resource_version: map.resource_version(),
                        }),
                        ..Default::default()
                    },
                )
                .await?;
            }
        }
        Ok(())
    }
    async fn node_admission_capability(&self) -> Result<()> {
        let hooks: Api<
            k8s_openapi::api::admissionregistration::v1::ValidatingWebhookConfiguration,
        > = Api::all(self.client.clone());
        let hook = hooks.get(&self.name("retention")).await?;
        ensure!(
            hook.labels().get(LABEL) == Some(&self.options.installation),
            "AdmissionOwnershipConflict"
        );
        let ca = hook
            .webhooks
            .as_ref()
            .and_then(|h| h.first())
            .and_then(|h| h.client_config.ca_bundle.as_ref())
            .context("MissingAdmissionCa")?;
        let dns = format!("{}.{}.svc", self.name("admission"), self.options.namespace);
        let tunnel = crate::tunnel::Tunnel::open(
            self.client.clone(),
            &self.options.namespace,
            &self.name("admission"),
            8443,
        )
        .await?;
        let client = reqwest::Client::builder()
            .https_only(true)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .add_root_certificate(reqwest::Certificate::from_pem(&ca.0)?)
            .resolve(&dns, tunnel.address)
            .timeout(Duration::from_secs(15))
            .build()?;
        let response = client
            .get(format!(
                "https://{dns}:{}/capabilities",
                tunnel.address.port()
            ))
            .send()
            .await?;
        ensure!(
            response.status().is_success(),
            "AdmissionUpgradeRequiredForNodeEndpoints"
        );
        ensure!(
            response.content_length().is_some_and(|n| n <= 4096),
            "InvalidAdmissionCapabilities"
        );
        let bytes = response.bytes().await?;
        ensure!(bytes.len() <= 4096, "InvalidAdmissionCapabilities");
        let value: serde_json::Value = serde_json::from_slice(&bytes)?;
        ensure!(
            value["nodeEndpointSchema"] == 1,
            "AdmissionUpgradeRequiredForNodeEndpoints"
        );
        Ok(())
    }
    /// Bootstrap only the target node's read proxy, warm it through a regular
    /// kubelet pull, then publish endpoint eligibility after ordinary handover.
    pub async fn join_node(&self, bundle: &Bundle, target: &str, recover: bool) -> Result<()> {
        ensure!(
            kimage_core::selector::dns_subdomain(target),
            "InvalidNodeName"
        );
        let mut options = self.options.clone();
        options.node = target.into();
        let target_inst = Installer::new(self.client.clone(), options)?;
        target_inst.preflight().await?;
        ensure!(
            target != self.options.node,
            "UsePrimaryRecoveryForRegistryNode"
        );
        let node: Node = Api::all(self.client.clone()).get(target).await?;
        let uid = node.uid().context("MissingNodeUid")?;
        let prefix = scope(&self.options.installation, &uid);
        let name = format!("{prefix}-proxy");
        let image = format!(
            "127.0.0.1:{}/{}/system-proxy@{}",
            self.options.port,
            self.options.namespace,
            bundle
                .inventory
                .components
                .get("proxy")
                .context("MissingProxyComponent")?
                .root
                .digest
        );
        let endpoint = Endpoint {
            schema: 1,
            node: target.into(),
            node_uid: uid.clone(),
            port: self.options.port,
            origin: self.origin(),
            namespaces: self
                .allowed_namespaces()
                .split(',')
                .map(str::to_owned)
                .collect(),
            proxy_images: BTreeSet::from([image.clone()]),
        };
        // Compare against the installed reader identity; never replace it on join.
        let secrets: Api<k8s_openapi::api::core::v1::Secret> =
            Api::namespaced(self.client.clone(), &self.options.namespace);
        let credentials = secrets.get(&self.name("proxy-credentials")).await?;
        ensure!(
            credentials.labels().get(LABEL) == Some(&self.options.installation),
            "ProxyCredentialOwnershipConflict"
        );
        for key in ["ca.crt", "reader-password"] {
            ensure!(
                credentials
                    .data
                    .as_ref()
                    .and_then(|d| d.get(key))
                    .is_some_and(|b| std::fs::read(self.options.credentials.join(key))
                        .is_ok_and(|v| v == b.0)),
                "ProxyCredentialMismatch"
            );
        }
        self.node_admission_capability().await?;
        self.readiness_permissions(Some(target)).await?;
        let component = bundle
            .inventory
            .components
            .get("proxy")
            .context("MissingProxyComponent")?;
        let verified = kimage_core::oci::verify_archive(
            &bundle.path(&component.archive)?,
            &component.root,
            &bundle.inventory.platform,
            kimage_core::oci::Limits::default(),
        )?;
        let tunnel = crate::tunnel::Tunnel::open(
            self.client.clone(),
            &self.options.namespace,
            &self.name("registry"),
            5443,
        )
        .await?;
        let registry = crate::registry::Registry::new(
            self.origin().parse()?,
            format!("{}/system-proxy", self.options.namespace),
            &std::fs::read(self.options.credentials.join("ca.crt"))?,
            Some(tunnel.address),
            "publisher".into(),
            std::fs::read_to_string(self.options.credentials.join("publisher-password"))?
                .trim_end()
                .into(),
        )?;
        registry
            .publish(&verified, &tokio_util::sync::CancellationToken::new())
            .await?;
        drop(tunnel);

        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.options.namespace);
        let page = pods
            .list(
                &ListParams::default()
                    .labels(&format!(
                        "{LABEL}={},kimage.dev/role=node-proxy",
                        self.options.installation
                    ))
                    .limit(128),
            )
            .await?;
        ensure!(
            page.metadata
                .continue_
                .as_ref()
                .is_none_or(|s| s.is_empty()),
            "TooManyNodeProxies"
        );
        for stale in page {
            if stale.spec.as_ref().and_then(|s| s.node_name.as_deref()) == Some(target)
                && stale.annotations().get("kimage.dev/node-uid") != Some(&uid)
            {
                let old_uid = stale
                    .annotations()
                    .get("kimage.dev/node-uid")
                    .context("MissingNodeUid")?;
                ensure!(recover, "NodeIdentityChangedUseRecover");
                ensure!(
                    stale.name_any()
                        == format!("{}-proxy", scope(&self.options.installation, old_uid)),
                    "NodeProxyOwnershipConflict"
                );
                self.delete_node_pod(&stale.name_any(), stale.uid()).await?;
            }
        }
        if let Some(existing) = pods.get_opt(&name).await? {
            if proxy_matches(&existing, &endpoint, &self.options.installation) && ready(&existing) {
                self.approve_endpoint(&endpoint).await?;
                self.cleanup_node_payload(bundle, &prefix).await?;
                return Ok(());
            }
            ensure!(
                existing.labels().get(LABEL) == Some(&self.options.installation)
                    && existing.annotations().get("kimage.dev/node-uid") == Some(&uid),
                "NodeProxyOwnershipConflict"
            );
            let carrier = existing.spec.as_ref().is_some_and(|s| {
                s.containers.len() == 1
                    && s.containers[0].image.as_deref() == Some(&self.options.carrier)
            });
            ensure!(
                carrier
                    || (recover && proxy_matches(&existing, &endpoint, &self.options.installation)),
                "NodeProxyBundleConflict"
            );
            if !carrier {
                self.delete_node_pod(&name, existing.uid()).await?;
            }
        }
        let launcher: ConfigMap = serde_json::from_value(
            json!({"apiVersion":"v1","kind":"ConfigMap","metadata":self.metadata(&format!("{prefix}-launcher")),"immutable":true,"binaryData":{"launcher":STANDARD.encode(std::fs::read(bundle.path(&bundle.inventory.launcher)?)?)}}),
        )?;
        self.owned(serde_json::to_value(launcher)?).await?;
        let (payload, count, digest) = self.payload_scoped(bundle, "proxy", &prefix).await?;
        let args = vec![
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
        let mut bootstrap_args = vec![
            "/payload/proxy".into(),
            count.to_string(),
            digest,
            "/work/proxy".into(),
        ];
        bootstrap_args.extend(args.clone());
        let mounts = vec![
            json!({"name":"launcher","mountPath":"/bootstrap","readOnly":true}),
            json!({"name":"proxy-payload","mountPath":"/payload/proxy","readOnly":true}),
            json!({"name":"work","mountPath":"/work"}),
            json!({"name":"credentials","mountPath":"/credentials","readOnly":true}),
        ];
        let volumes = vec![
            payload,
            json!({"name":"launcher","configMap":{"name":format!("{prefix}-launcher"),"defaultMode":365}}),
            json!({"name":"work","emptyDir":{"sizeLimit":"128Mi"}}),
            json!({"name":"credentials","secret":{"secretName":self.name("proxy-credentials"),"defaultMode":256}}),
        ];
        let mut carrier = target_inst.pod(
            "proxy",
            vec![self.container("proxy", bootstrap_args, mounts)],
            volumes,
            true,
        );
        carrier["metadata"] = self.metadata(&name);
        carrier["metadata"]["labels"]["kimage.dev/role"] = json!("node-proxy");
        carrier["metadata"]["annotations"]["kimage.dev/node-uid"] = json!(uid);
        carrier["spec"]["containers"][0]["readinessProbe"] = json!({"httpGet":{"host":"127.0.0.1","path":"/v2/","port":self.options.port},"periodSeconds":2,"timeoutSeconds":3});
        self.owned(carrier.clone()).await?;
        self.wait_pod(&name).await?;
        let warm_name = format!("{prefix}-w-{}", &component.root.digest[7..15]);
        self.owned(json!({"apiVersion":"v1","kind":"Pod","metadata":self.metadata(&warm_name),"spec":{"nodeName":target,"automountServiceAccountToken":false,"restartPolicy":"Never","containers":[{"name":"warmup","image":image,"imagePullPolicy":"IfNotPresent","command":["/kimage-proxy","--help"],"securityContext":{"allowPrivilegeEscalation":false,"readOnlyRootFilesystem":true,"capabilities":{"drop":["ALL"]}},"resources":{"requests":{"cpu":"10m","memory":"16Mi"},"limits":{"cpu":"500m","memory":"128Mi"}}}]}})).await?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
        loop {
            let pod = pods.get(&warm_name).await?;
            let phase = pod.status.as_ref().and_then(|s| s.phase.as_deref());
            if phase == Some("Succeeded") {
                break;
            }
            ensure!(phase != Some("Failed"), "NodeProxyWarmupFailed");
            ensure!(
                tokio::time::Instant::now() < deadline,
                "NodeProxyWarmupTimeout"
            );
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        let old = pods.get(&name).await?;
        self.delete_node_pod(&name, old.uid()).await?;
        let container = &mut carrier["spec"]["containers"][0];
        container["image"] = json!(image);
        container["imagePullPolicy"] = json!("IfNotPresent");
        container["command"] = json!(["/kimage-proxy"]);
        container["args"] = json!(args);
        container["volumeMounts"] =
            json!([{"name":"credentials","mountPath":"/credentials","readOnly":true}]);
        carrier["spec"]["volumes"] = json!([{"name":"credentials","secret":{"secretName":self.name("proxy-credentials"),"defaultMode":256}}]);
        self.owned(carrier).await?;
        self.wait_pod(&name).await?;
        self.approve_endpoint(&endpoint).await?;
        let warm = pods.get(&warm_name).await?;
        self.delete_node_pod(&warm_name, warm.uid()).await?;
        self.cleanup_node_payload(bundle, &prefix).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn endpoint() -> Endpoint {
        Endpoint {
            schema: 1,
            node: "worker".into(),
            node_uid: "uid".into(),
            port: 17641,
            origin: "https://test-registry.system.svc:5443/".into(),
            namespaces: BTreeSet::from(["system".into(), "app".into()]),
            proxy_images: BTreeSet::from([format!(
                "127.0.0.1:17641/system/system-proxy@{}",
                kimage_core::oci::digest(b"proxy")
            )]),
        }
    }
    #[test]
    fn endpoint_approval_binds_scope_and_pinned_proxy_not_pod_labels() {
        let endpoint = endpoint();
        endpoint.validate("test", "system", "app", 17641).unwrap();
        let mut pod:Pod=serde_json::from_value(json!({"metadata":{"name":format!("{}-proxy",scope("test","uid")),"labels":{LABEL:"test"},"annotations":{"kimage.dev/node-uid":"uid"}},"spec":{"nodeName":"worker","hostNetwork":true,"automountServiceAccountToken":false,"containers":[{"name":"proxy","image":endpoint.proxy_images.first().unwrap(),"command":["/kimage-proxy"]}]}})).unwrap();
        let mut value = serde_json::to_value(&pod).unwrap();
        value["spec"]["dnsPolicy"] = json!("ClusterFirstWithHostNet");
        value["spec"]["securityContext"] = json!({"seccompProfile":{"type":"RuntimeDefault"}});
        value["spec"]["containers"][0]["imagePullPolicy"] = json!("IfNotPresent");
        value["spec"]["containers"][0]["securityContext"] = json!({"allowPrivilegeEscalation":false,"readOnlyRootFilesystem":true,"capabilities":{"drop":["ALL"]}});
        value["spec"]["containers"][0]["args"] = json!([
            "--listen",
            "127.0.0.1:17641",
            "--upstream",
            endpoint.origin,
            "--ca",
            "/credentials/ca.crt",
            "--password-file",
            "/credentials/reader-password",
            "--namespaces",
            "system,app"
        ]);
        value["spec"]["containers"][0]["readinessProbe"] =
            json!({"httpGet":{"host":"127.0.0.1","path":"/v2/","port":17641}});
        value["spec"]["containers"][0]["volumeMounts"] =
            json!([{"name":"credentials","mountPath":"/credentials","readOnly":true}]);
        value["spec"]["volumes"] =
            json!([{"name":"credentials","secret":{"secretName":"test-proxy-credentials"}}]);
        pod = serde_json::from_value(value).unwrap();
        assert!(proxy_matches(&pod, &endpoint, "test"));
        pod.spec.as_mut().unwrap().containers[0].image = Some("unapproved".into());
        assert!(!proxy_matches(&pod, &endpoint, "test"));
        pod.spec.as_mut().unwrap().containers[0].image = endpoint.proxy_images.first().cloned();
        pod.metadata
            .annotations
            .as_mut()
            .unwrap()
            .insert("kimage.dev/node-uid".into(), "old-uid".into());
        assert!(!proxy_matches(&pod, &endpoint, "test"));
        let mut wrong = endpoint.clone();
        wrong.origin = "https://unapproved.invalid/".into();
        assert!(wrong.validate("test", "system", "app", 17641).is_err());
        let mut wrong = endpoint;
        wrong.namespaces.insert("other".into());
        assert!(wrong.validate("test", "system", "app", 17641).is_err());
        assert!(format!("{}-proxy-next", scope(&"a".repeat(32), "node-uid")).len() <= 63);
    }
}

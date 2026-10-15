//! Ordinary-image control plane. The installer activates admission only after
//! operator-owned schemas and a verified TLS serving endpoint are ready.
use crate::{bundle::Bundle, install::Installer};
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use kube::{Api, ResourceExt};
use serde_json::{Value, json};
use std::path::Path;

fn rule(group: &str, resources: &[&str], verbs: &[&str]) -> Value {
    json!({"apiGroups":[group],"resources":resources,"verbs":verbs})
}
fn named_rule(group: &str, resources: &[&str], verbs: &[&str], names: &[&str]) -> Value {
    let mut r = rule(group, resources, verbs);
    r["resourceNames"] = json!(names);
    r
}
fn cluster_rules() -> Vec<Value> {
    vec![
        // Kubernetes cannot limit top-level CREATE by resourceNames.
        rule(
            "apiextensions.k8s.io",
            &["customresourcedefinitions"],
            &["create"],
        ),
        named_rule(
            "apiextensions.k8s.io",
            &["customresourcedefinitions"],
            &["get", "update", "patch"],
            &[
                "images.kimage.dev",
                "imagereleases.kimage.dev",
                "imagedemands.kimage.dev",
            ],
        ),
        rule("", &["persistentvolumes"], &["get"]),
    ]
}
fn application_rules(admission: bool) -> Vec<Value> {
    if admission {
        return vec![rule(
            "kimage.dev",
            &["images", "imagereleases"],
            &["get", "list"],
        )];
    }
    vec![
        rule(
            "kimage.dev",
            &["images"],
            &["get", "list", "watch", "create", "patch"],
        ),
        rule(
            "kimage.dev",
            &["images/status", "imagereleases/status"],
            &["get", "patch"],
        ),
        rule(
            "kimage.dev",
            &["imagereleases"],
            &["get", "list", "create", "patch", "delete"],
        ),
        rule("", &["persistentvolumeclaims"], &["get"]),
        rule("", &["pods"], &["get", "list", "create", "delete"]),
        rule("", &["pods/exec"], &["get", "create"]),
        rule(
            "",
            &["configmaps"],
            &["get", "list", "watch", "create", "delete"],
        ),
        rule(
            "",
            &["secrets"],
            &["get", "list", "watch", "create", "patch", "delete"],
        ),
    ]
}
impl Installer {
    pub(crate) async fn ensure_scope_available(&self) -> Result<()> {
        use k8s_openapi::api::admissionregistration::v1::MutatingWebhookConfiguration;
        let api: Api<MutatingWebhookConfiguration> = Api::all(self.client.clone());
        let hooks = api
            .list(
                &kube::api::ListParams::default()
                    .labels(crate::install::LABEL)
                    .limit(256),
            )
            .await?;
        ensure!(
            hooks
                .metadata
                .continue_
                .as_ref()
                .is_none_or(|v| v.is_empty()),
            "TooManyInstallationScopes"
        );
        for hook in hooks {
            let Some(webhooks) = &hook.webhooks else {
                continue;
            };
            for webhook in webhooks {
                let Some(service) = &webhook.client_config.service else {
                    anyhow::bail!("UnknownInstallationScope");
                };
                if hook.name_any() == self.name("resolution")
                    && service.namespace == self.options.namespace
                {
                    continue;
                }
                let other = if let Some(scope) = hook.annotations().get("kimage.dev/scope") {
                    serde_json::from_str::<kimage_core::scope::NamespaceScope>(scope)?
                } else {
                    let application = webhook
                        .namespace_selector
                        .as_ref()
                        .and_then(|s| s.match_labels.as_ref())
                        .and_then(|l| l.get("kubernetes.io/metadata.name"))
                        .context("UnknownInstallationScope")?;
                    kimage_core::scope::NamespaceScope {
                        system_namespace: service.namespace.clone(),
                        application_namespace: Some(application.clone()),
                        cluster_wide: false,
                        excluded_namespaces: Default::default(),
                    }
                };
                ensure!(
                    !self.scope().overlaps(&other),
                    "InstallationScopeOverlap: exclude the existing application's namespace or use a separate scope"
                );
            }
        }
        Ok(())
    }
    pub(crate) fn reference(&self, bundle: &Bundle, role: &str) -> Result<String> {
        let c = bundle
            .inventory
            .components
            .get(role)
            .context("MissingControlComponent")?;
        Ok(format!(
            "127.0.0.1:{}/{}/system-{role}@{}",
            self.options.port, self.options.namespace, c.root.digest
        ))
    }
    fn binding(&self, role: &str, kind: &str) -> Value {
        json!({"apiVersion":"rbac.authorization.k8s.io/v1","kind":kind,"metadata":self.metadata(&self.name(role)),
            "subjects":[{"kind":"ServiceAccount","name":self.name(role),"namespace":self.options.namespace}],
            "roleRef":{"apiGroup":"rbac.authorization.k8s.io","kind":if kind == "ClusterRoleBinding" {"ClusterRole"} else {"Role"},"name":self.name(role)}})
    }
    async fn permissions(&self, application: &str) -> Result<()> {
        for role in ["operator", "admission"] {
            self.owned(json!({"apiVersion":"v1","kind":"ServiceAccount","metadata":self.metadata(&self.name(role)),"automountServiceAccountToken":false})).await?;
            if self.scope().cluster_wide {
                let name = self.name(&format!("application-{role}"));
                self.owned_at(json!({"apiVersion":"rbac.authorization.k8s.io/v1","kind":"ClusterRole","metadata":self.metadata(&name),"rules":application_rules(role=="admission")}),None).await?;
                self.owned_at(json!({"apiVersion":"rbac.authorization.k8s.io/v1","kind":"ClusterRoleBinding","metadata":self.metadata(&name),"subjects":[{"kind":"ServiceAccount","name":self.name(role),"namespace":self.options.namespace}],"roleRef":{"apiGroup":"rbac.authorization.k8s.io","kind":"ClusterRole","name":name}}),None).await?;
            } else {
                self.owned_at(json!({"apiVersion":"rbac.authorization.k8s.io/v1","kind":"Role","metadata":self.metadata(&self.name(role)),"rules":application_rules(role == "admission")}), Some(application)).await?;
                self.owned_at(self.binding(role, "RoleBinding"), Some(application))
                    .await?;
            }
        }
        let mut cluster = cluster_rules();
        cluster.push(rule("", &["nodes"], &["get", "list"]));
        self.owned_at(json!({"apiVersion":"rbac.authorization.k8s.io/v1","kind":"ClusterRole","metadata":self.metadata(&self.name("operator")),"rules":cluster}), None).await?;
        let binding = self.binding("operator", "ClusterRoleBinding");
        self.owned_at(binding, None).await?;
        self.readiness_permissions(None).await?;
        self.owned(self.binding("admission", "RoleBinding")).await?;
        self.owned_at(self.binding("admission", "ClusterRoleBinding"), None)
            .await?;
        // Schema coordination remains built-in, independently usable before CRDs.
        let mut rules = Vec::new();
        for (group, resource, name) in [
            ("coordination.k8s.io", "leases", "kimage-schema-manager"),
            ("", "configmaps", "kimage-schema-status"),
        ] {
            rules.push(rule(group, &[resource], &["create"]));
            rules.push(named_rule(
                group,
                &[resource],
                &["get", "update", "patch"],
                &[name],
            ));
        }
        rules.extend([
            rule("", &["configmaps"], &["get", "list", "watch"]),
            rule("kimage.dev", &["imagedemands"], &["get", "list", "watch"]),
            named_rule(
                "",
                &["secrets"],
                &["get"],
                &[
                    &self.name("builder-client"),
                    &self.name("builder-client-amd64"),
                    &self.name("builder-client-arm64"),
                ],
            ),
            named_rule(
                "",
                &["pods"],
                &["get", "delete"],
                &[
                    &self.name("builder"),
                    &self.name("builder-amd64"),
                    &self.name("builder-arm64"),
                ],
            ),
            rule("", &["pods"], &["create"]),
            rule("discovery.k8s.io", &["endpointslices"], &["get", "list"]),
            named_rule(
                "coordination.k8s.io",
                &["leases"],
                &["get", "update", "patch"],
                &[
                    &self.name("direct-worker"),
                    &self.name("direct-worker-amd64"),
                    &self.name("direct-worker-arm64"),
                ],
            ),
        ]);
        self.owned(json!({"apiVersion":"rbac.authorization.k8s.io/v1","kind":"Role","metadata":self.metadata(&self.name("operator")),"rules":rules})).await?;
        let binding = self.binding("operator", "RoleBinding");
        self.owned(binding).await
    }
    pub async fn control_plane(&self, bundle: &Bundle, credentials: &Path) -> Result<()> {
        ensure!(
            bundle.inventory.api_schema_revision == Some(crate::schema::REVISION),
            "BundleApiSchemaMismatch: use its matching retained installer"
        );
        self.scope().validate()?;
        self.ensure_scope_available().await?;
        let app = self.options.application_namespace.as_deref().unwrap_or("*");
        // Refuse an incompatible installed schema before any control-plane writes.
        let api: Api<k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition> = Api::all(self.client.clone());
        for desired in crate::schema::definitions()? {
            if let Some(current) = api.get_opt(&desired.name_any()).await? {
                crate::schema::compatibility(&current, &desired)?;
            }
        }
        let mut admission_credentials = serde_json::Map::new();
        for name in ["ca.crt", "tls.crt", "tls.key", "resolution-key"] {
            let path = credentials.join(name);
            let meta = std::fs::symlink_metadata(&path)?;
            ensure!(
                meta.is_file() && meta.len() > 0 && meta.len() <= 65536,
                "InvalidAdmissionCredentialFile"
            );
            let bytes = std::fs::read(path)?;
            if name == "resolution-key" {
                ensure!(bytes.len() >= 32, "InvalidResolutionKey");
            }
            admission_credentials.insert(name.into(), json!(STANDARD.encode(bytes)));
        }
        self.state_named(bundle, "ControlPlanePending", "control-state")
            .await?;
        self.ensure_configuration().await?;
        self.permissions(app).await?;
        // Operator never receives a registry reader password or TLS private key.
        self.owned(json!({"apiVersion":"v1","kind":"Secret","metadata":self.metadata(&self.name("publisher-credentials")),"type":"Opaque","data":{
            "ca.crt":STANDARD.encode(std::fs::read(self.options.credentials.join("ca.crt"))?),
            "publisher-password":STANDARD.encode(std::fs::read(self.options.credentials.join("publisher-password"))?)}})).await?;
        self.owned(json!({"apiVersion":"v1","kind":"Secret","metadata":self.metadata(&self.name("admission-credentials")),"type":"Opaque","data":admission_credentials})).await?;
        let input = self.reference(bundle, "input")?;
        for role in ["operator", "admission"] {
            // Starting admission is deferred until the operator's schema phase passed.
            if role == "admission" {
                self.wait_pod(&self.name("operator")).await?;
                for desired in crate::schema::definitions()? {
                    crate::schema::compatibility(&api.get(&desired.name_any()).await?, &desired)?;
                }
            }
            let mut arguments = if role == "operator" {
                vec![
                    "--namespace".into(),
                    app.into(),
                    "--system-namespace".into(),
                    self.options.namespace.clone(),
                    "--node".into(),
                    self.options.node.clone(),
                    "--input-image".into(),
                    input.clone(),
                    "--registry-origin".into(),
                    self.origin(),
                    "--registry-ca".into(),
                    "/credentials/ca.crt".into(),
                    "--publisher-password".into(),
                    "/credentials/publisher-password".into(),
                ]
            } else {
                vec![
                    "--namespace".into(),
                    app.into(),
                    "--system-namespace".into(),
                    self.options.namespace.clone(),
                    "--installation".into(),
                    self.options.installation.clone(),
                    "--endpoint".into(),
                    format!("127.0.0.1:{}", self.options.port),
                    "--nodes".into(),
                    self.options.node.clone(),
                    "--cert".into(),
                    "/credentials/tls.crt".into(),
                    "--key".into(),
                    "/credentials/tls.key".into(),
                    "--resolution-key".into(),
                    "/credentials/resolution-key".into(),
                    "--operator-identity".into(),
                    format!(
                        "system:serviceaccount:{}:{}",
                        self.options.namespace,
                        self.name("operator")
                    ),
                    "--input-image".into(),
                    input.clone(),
                ]
            };
            if bundle.inventory.components.contains_key("builder") {
                arguments.extend(["--builder-image".into(), self.reference(bundle, "builder")?]);
                if role == "operator" {
                    arguments.extend(["--installation".into(), self.options.installation.clone()]);
                }
            }
            if self.scope().cluster_wide {
                arguments.drain(0..2);
                arguments.push("--cluster-wide".into());
                for namespace in &self.options.excluded_namespaces {
                    arguments.extend(["--exclude-namespace".into(), namespace.clone()]);
                }
            }
            let container = json!({"name":role,"image":self.reference(bundle, role)?,"imagePullPolicy":"IfNotPresent","command":[format!("/kimage-{role}")],"args":arguments,
                "env":[{"name":"POD_UID","valueFrom":{"fieldRef":{"fieldPath":"metadata.uid"}}}],
                "volumeMounts":[{"name":"credentials","mountPath":"/credentials","readOnly":true},{"name":"scratch","mountPath":"/tmp"}],
                "resources":{"requests":{"cpu":"50m","memory":"64Mi"},"limits":{"cpu":"1","memory":"512Mi","ephemeral-storage":"2Gi"}},
                "securityContext":{"runAsUser":65532,"runAsGroup":65532,"runAsNonRoot":true,"allowPrivilegeEscalation":false,"readOnlyRootFilesystem":true,"capabilities":{"drop":["ALL"]}},
                "readinessProbe":{"httpGet":{"path":"/readyz","port":if role == "operator" {8080} else {8443},"scheme":if role == "operator" {"HTTP"} else {"HTTPS"}},"periodSeconds":2}});
            let volumes = vec![
                json!({"name":"credentials","secret":{"secretName":self.name(if role == "operator" {"publisher-credentials"} else {"admission-credentials"}),"defaultMode":292}}),
                json!({"name":"scratch","emptyDir":{"sizeLimit":"2Gi"}}),
            ];
            let mut pod = self.pod(role, vec![container], volumes, false);
            pod["spec"]["serviceAccountName"] = json!(self.name(role));
            pod["spec"]["automountServiceAccountToken"] = json!(true);
            self.owned(pod).await?;
        }
        self.wait_pod(&self.name("admission")).await?;
        self.owned(json!({"apiVersion":"v1","kind":"Service","metadata":self.metadata(&self.name("admission")),"spec":{"selector":{"kimage.dev/installation":self.options.installation,"kimage.dev/role":"admission"},"ports":[{"port":443,"targetPort":8443}]}})).await?;
        let tunnel = crate::tunnel::Tunnel::open(
            self.client.clone(),
            &self.options.namespace,
            &self.name("admission"),
            8443,
        )
        .await?;
        let hostname = format!("{}.{}.svc", self.name("admission"), self.options.namespace);
        let ca = std::fs::read(credentials.join("ca.crt"))?;
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(10))
            .add_root_certificate(reqwest::Certificate::from_pem(&ca)?)
            .resolve(&hostname, tunnel.address)
            .build()?;
        client
            .get(format!(
                "https://{hostname}:{}/readyz",
                tunnel.address.port()
            ))
            .send()
            .await
            .context("AdmissionTlsReadinessFailed")?
            .error_for_status()?;
        self.owned_at(self.webhook(app, &ca), None).await?;
        let mut retention = self.webhook(app, &ca);
        retention["webhooks"][0]["sideEffects"] = json!("None");
        retention["kind"] = json!("ValidatingWebhookConfiguration");
        retention["metadata"]["name"] = json!(self.name("retention"));
        retention["webhooks"][0]["name"] = json!(format!(
            "{}.retention.kimage.dev",
            self.options.installation
        ));
        retention["webhooks"][0]
            .as_object_mut()
            .unwrap()
            .remove("reinvocationPolicy");
        retention["webhooks"][0]["clientConfig"]["service"]["path"] = json!("/retain");
        retention["webhooks"][0]["rules"] = json!([{"apiGroups":["kimage.dev"],"apiVersions":["v1alpha1"],"operations":["CREATE","DELETE","UPDATE"],"resources":["imagereleases"],"scope":"Namespaced"}]);
        self.owned_at(retention, None).await?;
        self.state_named(bundle, "ControlPlaneReady", "control-state")
            .await
    }
    fn webhook(&self, _application: &str, ca: &[u8]) -> Value {
        json!({"apiVersion":"admissionregistration.k8s.io/v1","kind":"MutatingWebhookConfiguration","metadata":{"name":self.name("resolution"),"labels":{crate::install::LABEL:self.options.installation},"annotations":{"kimage.dev/scope":serde_json::to_string(&self.scope()).expect("scope serialization")}},"webhooks":[{
            "name":format!("{}.kimage.dev", self.options.installation),"admissionReviewVersions":["v1"],"sideEffects":"NoneOnDryRun","failurePolicy":"Fail","matchPolicy":"Equivalent","reinvocationPolicy":"IfNeeded","timeoutSeconds":5,
            "namespaceSelector":self.scope().webhook_selector(),
            "clientConfig":{"service":{"name":self.name("admission"),"namespace":self.options.namespace,"path":"/mutate","port":443},"caBundle":STANDARD.encode(ca)},
            "rules":[{"apiGroups":[""],"apiVersions":["v1"],"operations":["CREATE","UPDATE"],"resources":["pods","pods/ephemeralcontainers"],"scope":"Namespaced"}]}]})
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identities_have_disjoint_read_and_schema_permissions() {
        let admission = application_rules(true);
        assert!(admission.iter().all(|r| {
            r["verbs"]
                .as_array()
                .unwrap()
                .iter()
                .all(|v| v == "get" || v == "list")
        }));
        assert!(
            admission
                .iter()
                .all(|r| r["apiGroups"] == json!(["kimage.dev"]))
        );
        for r in cluster_rules() {
            assert!(!r["verbs"].as_array().unwrap().contains(&json!("delete")));
            if r["apiGroups"] == json!(["apiextensions.k8s.io"]) && r["verbs"] != json!(["create"])
            {
                assert_eq!(
                    r["resourceNames"],
                    json!([
                        "images.kimage.dev",
                        "imagereleases.kimage.dev",
                        "imagedemands.kimage.dev"
                    ])
                );
            }
        }
    }
}

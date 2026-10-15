//! Explicit optional exporter identity; no worker, node or CRD-management access.
use crate::{bundle::Bundle, install::Installer};
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::json;
use std::collections::BTreeSet;
impl Installer {
    pub async fn exporter(&self, bundle: &Bundle, approved: &[String]) -> Result<()> {
        ensure!(
            bundle.inventory.api_schema_revision == Some(crate::schema::REVISION),
            "BundleApiSchemaMismatch"
        );
        let component = bundle
            .inventory
            .components
            .get("exporter")
            .context("MissingExporterComponent")?;
        let app = self.options.application_namespace.as_deref().unwrap_or("*");
        self.scope().validate()?;
        ensure!(
            !approved.is_empty() && approved.len() <= 8,
            "ExplicitExportOriginsRequired"
        );
        let mut origins = BTreeSet::new();
        for value in approved {
            let origin: reqwest::Url = value.parse()?;
            ensure!(
                origin.scheme() == "https"
                    && origin.username().is_empty()
                    && origin.password().is_none()
                    && origin.path() == "/"
                    && origin.query().is_none()
                    && origin.fragment().is_none(),
                "InvalidApprovedExportOrigin"
            );
            origins.insert(origin.to_string());
        }
        let role = self.name("exporter");
        let reader = self.name("export-reader");
        let mut data = serde_json::Map::new();
        for name in ["ca.crt", "reader-password"] {
            let bytes = std::fs::read(self.options.credentials.join(name))?;
            ensure!(
                !bytes.is_empty() && bytes.len() <= 65536,
                "InvalidExporterReaderCredential"
            );
            data.insert(name.into(), json!(STANDARD.encode(bytes)));
        }
        self.owned(json!({"apiVersion":"v1","kind":"Secret","metadata":self.metadata(&reader),"type":"Opaque","data":data})).await?;
        self.owned(json!({"apiVersion":"v1","kind":"ServiceAccount","metadata":self.metadata(&role),"automountServiceAccountToken":false})).await?;
        let rules = json!([{"apiGroups":["kimage.dev"],"resources":["images","imagereleases"],"verbs":["get","list","watch"]},{"apiGroups":["kimage.dev"],"resources":["images/status"],"verbs":["patch"]},{"apiGroups":[""],"resources":["secrets"],"verbs":["get"]}]);
        let binding = json!({"apiVersion":"rbac.authorization.k8s.io/v1","kind":"RoleBinding","metadata":self.metadata(&role),"subjects":[{"kind":"ServiceAccount","name":role,"namespace":self.options.namespace}],"roleRef":{"apiGroup":"rbac.authorization.k8s.io","kind":"Role","name":role}});
        if self.scope().cluster_wide {
            let name = self.name("application-exporter");
            self.owned_at(json!({"apiVersion":"rbac.authorization.k8s.io/v1","kind":"ClusterRole","metadata":self.metadata(&name),"rules":rules}),None).await?;
            let mut cluster_binding = binding.clone();
            cluster_binding["kind"] = json!("ClusterRoleBinding");
            cluster_binding["metadata"] = self.metadata(&name);
            cluster_binding["roleRef"]["kind"] = json!("ClusterRole");
            cluster_binding["roleRef"]["name"] = json!(name);
            self.owned_at(cluster_binding, None).await?;
        } else {
            self.owned_at(json!({"apiVersion":"rbac.authorization.k8s.io/v1","kind":"Role","metadata":self.metadata(&role),"rules":rules}),Some(app)).await?;
            self.owned_at(binding.clone(), Some(app)).await?;
        }
        self.owned(json!({"apiVersion":"rbac.authorization.k8s.io/v1","kind":"Role","metadata":self.metadata(&role),"rules":[{"apiGroups":["coordination.k8s.io"],"resources":["leases"],"verbs":["get","create","update"]}]})).await?;
        self.owned(binding).await?;
        let reference = format!(
            "127.0.0.1:{}/{}/system-exporter@{}",
            self.options.port, self.options.namespace, component.root.digest
        );
        let mut args = vec![
            "--namespace".to_owned(),
            app.into(),
            "--system-namespace".into(),
            self.options.namespace.clone(),
            "--installation".into(),
            self.options.installation.clone(),
            "--internal-origin".into(),
            self.origin(),
            "--internal-ca".into(),
            "/credentials/ca.crt".into(),
            "--internal-password".into(),
            "/credentials/reader-password".into(),
        ];
        if self.scope().cluster_wide {
            args.drain(..2);
            args.push("--cluster-wide".into());
            for ns in &self.options.excluded_namespaces {
                args.extend(["--exclude-namespace".into(), ns.clone()]);
            }
        }
        for origin in origins {
            args.extend(["--approved-origin".into(), origin]);
        }
        let mut meta = self.metadata(&role);
        meta["labels"]["kimage.dev/role"] = json!("exporter");
        self.owned(json!({"apiVersion":"v1","kind":"Pod","metadata":meta,"spec":{"nodeName":self.options.node,"serviceAccountName":role,"automountServiceAccountToken":true,"securityContext":{"runAsUser":65532,"runAsGroup":65532,"fsGroup":65532,"seccompProfile":{"type":"RuntimeDefault"}},"volumes":[{"name":"credentials","secret":{"secretName":reader,"defaultMode":288}},{"name":"temporary","emptyDir":{"sizeLimit":"4Gi"}}],"containers":[{"name":"exporter","image":reference,"imagePullPolicy":"IfNotPresent","command":["/kimage-exporter"],"args":args,"volumeMounts":[{"name":"credentials","mountPath":"/credentials","readOnly":true},{"name":"temporary","mountPath":"/tmp"}],"securityContext":{"allowPrivilegeEscalation":false,"readOnlyRootFilesystem":true,"capabilities":{"drop":["ALL"]}},"readinessProbe":{"httpGet":{"path":"/readyz","port":8080},"timeoutSeconds":3},"resources":{"requests":{"cpu":"100m","memory":"128Mi"},"limits":{"cpu":"1","memory":"512Mi"}}}]}})).await?;
        self.wait_pod(&role).await
    }
}

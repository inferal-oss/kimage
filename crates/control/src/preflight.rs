//! Namespace/authorization/policy prerequisites before bootstrap data writes.
use crate::install::Installer;
use anyhow::{Result, ensure};
use k8s_openapi::api::{authorization::v1::SelfSubjectAccessReview, core::v1::Namespace};
use kube::{Api, api::PostParams};
use serde_json::json;
impl Installer {
    pub(crate) async fn ensure_namespaces(&self) -> Result<()> {
        let api: Api<Namespace> = Api::all(self.client.clone());
        let mut namespaces = std::collections::BTreeSet::from([self.options.namespace.as_str()]);
        if let Some(app) = &self.options.application_namespace {
            namespaces.insert(app);
        }
        for name in namespaces {
            if api.get_opt(name).await?.is_none() {
                let value: Namespace = serde_json::from_value(
                    json!({"apiVersion":"v1","kind":"Namespace","metadata":self.metadata(name)}),
                )?;
                api.create(
                    &PostParams {
                        dry_run: true,
                        ..Default::default()
                    },
                    &value,
                )
                .await?;
                api.create(&PostParams::default(), &value).await?;
            }
        }
        Ok(())
    }
    pub(crate) async fn bootstrap_policy_preflight(&self) -> Result<()> {
        let review_api: Api<SelfSubjectAccessReview> = Api::all(self.client.clone());
        for (resource, verbs) in [
            ("pods", vec!["create", "get", "delete"]),
            ("pods/portforward", vec!["create", "get"]),
            ("persistentvolumeclaims", vec!["create", "get"]),
            ("configmaps", vec!["create", "get", "update"]),
            ("secrets", vec!["create", "get"]),
            ("services", vec!["create", "get", "patch"]),
        ] {
            for verb in verbs {
                let (resource, subresource) = resource
                    .split_once('/')
                    .map_or((resource, None), |(r, s)| (r, Some(s)));
                let review: SelfSubjectAccessReview = serde_json::from_value(
                    json!({"apiVersion":"authorization.k8s.io/v1","kind":"SelfSubjectAccessReview","spec":{"resourceAttributes":{"namespace":self.options.namespace,"group":"","resource":resource,"subresource":subresource,"verb":verb}}}),
                )?;
                ensure!(
                    review_api
                        .create(&PostParams::default(), &review)
                        .await?
                        .status
                        .is_some_and(|s| s.allowed),
                    "BootstrapPermissionDenied: {verb} {resource}"
                );
            }
        }
        // API admission validates references without requiring their contents to
        // exist. Exercise both real bootstrap security/network profiles before
        // creating PVCs, Secrets, payload chunks or serving resources.
        for (role, host) in [("registry", false), ("proxy", true)] {
            let container = json!({"name":role,"image":self.options.carrier,"imagePullPolicy":"Never","command":["/bootstrap/launcher"],"args":["/payload","1","digest","--help"],"securityContext":{"runAsUser":0,"runAsGroup":0,"allowPrivilegeEscalation":false,"readOnlyRootFilesystem":true,"capabilities":{"drop":["ALL"]}},"resources":{"requests":{"cpu":"25m","memory":"64Mi"},"limits":{"cpu":"1","memory":"256Mi"}},"volumeMounts":[{"name":"credentials","mountPath":"/credentials","readOnly":true}]});
            let mut volumes = vec![
                json!({"name":"credentials","secret":{"secretName":self.name("registry-credentials"),"defaultMode":256}}),
            ];
            if !host {
                volumes.push(json!({"name":"data","persistentVolumeClaim":{"claimName":self.options.pvc.clone().unwrap_or_else(||self.name("data"))}}));
            }
            let pod = self.pod(&format!("preflight-{role}"), vec![container], volumes, host);
            let desired = serde_json::from_value(pod)?;
            self.api_at("Pod",Some(&self.options.namespace))?.create(&PostParams{dry_run:true,..Default::default()},&desired).await.map_err(|_|anyhow::anyhow!("BootstrapAdmissionDenied: namespace must permit the bounded root launcher and loopback proxy profile"))?;
        }
        Ok(())
    }
}

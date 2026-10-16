//! Deterministic Argo CD desired state; private material is resolved in cluster.
use crate::{
    component_images::ImageCatalog,
    install::LABEL,
    pull_install::{Options, credentials::Credentials},
};
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use k8s_openapi::api::core::v1::{ConfigMap, Secret};
use kube::{Api, Client, ResourceExt};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, RwLock},
    time::Duration,
};

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Desired {
    pub schema: u32,
    pub options: Options,
    pub catalog: ImageCatalog,
    #[serde(default)]
    pub credential_secrets: BTreeMap<String, String>,
    #[serde(default)]
    pub node_policy: Option<crate::node_reconcile::Policy>,
    #[serde(default)]
    pub build_policy: Option<String>,
}
impl Desired {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.schema == 1, "UnsupportedGitOpsConfiguration");
        self.options.validate()?;
        ensure!(
            self.options.worker_nodes.is_empty(),
            "UseNodePolicyForGitOpsWorkerPins"
        );
        self.catalog.validate(&self.catalog.release, false)?;
        let contract = crate::lifecycle::release_metadata()?;
        ensure!(
            self.catalog.release == env!("CARGO_PKG_VERSION")
                || contract["transitions"].as_array().is_some_and(|edges| edges
                    .iter()
                    .any(|edge| edge["from"] == self.catalog.release
                        || edge["to"] == self.catalog.release)),
            "UnsupportedReleaseTransition"
        );
        ensure!(
            kimage_core::selector::exact(&self.catalog.release)?
                >= kimage_core::selector::exact("0.5.1")?,
            "TargetDoesNotSupportArgoCD"
        );
        ensure!(
            self.build_policy
                .as_deref()
                .is_none_or(|p| matches!(p, "OnDemand" | "Eager")),
            "InvalidBuildPolicy"
        );
        ensure!(
            self.credential_secrets
                .iter()
                .all(
                    |(k, v)| ["registry", "admission", "builder"].contains(&k.as_str())
                        && !v.is_empty()
                        && v.len() <= 253
                        && v.bytes().all(|c| c.is_ascii_lowercase()
                            || c.is_ascii_digit()
                            || b".-".contains(&c))
                ),
            "InvalidCredentialSecretReference"
        );
        if let Some(policy) = &self.node_policy {
            crate::node_reconcile::validate_policy(policy)?;
        }
        Ok(())
    }
}

async fn material(client: Client, desired: &Desired) -> Result<Credentials> {
    let o = &desired.options;
    let mut credentials =
        Credentials::resolve(client.clone(), &o.namespace, &o.installation).await?;
    let secrets: Api<Secret> = Api::namespaced(client, &o.namespace);
    for (role, name) in &desired.credential_secrets {
        let secret = secrets.get(name).await?;
        ensure!(
            secret.metadata.deletion_timestamp.is_none(),
            "CredentialSecretDeleting"
        );
        let supplied = secret.data.context("MissingInstallationCredentials")?;
        let target = match role.as_str() {
            "registry" => &mut credentials.registry,
            "admission" => &mut credentials.admission,
            "builder" => &mut credentials.builder,
            _ => unreachable!(),
        };
        ensure!(
            supplied.keys().eq(target.keys())
                && supplied
                    .values()
                    .all(|v| !v.0.is_empty() && v.0.len() <= 65536),
            "InvalidInstallationCredentialKeys"
        );
        *target = supplied
            .into_iter()
            .map(|(k, v)| (k, STANDARD.encode(v.0)))
            .collect();
    }
    credentials.validate()?;
    Ok(credentials)
}

pub async fn reconcile(client: Client, desired: &Desired) -> Result<()> {
    desired.validate()?;
    let o = &desired.options;
    let maps: Api<ConfigMap> = Api::namespaced(client.clone(), &o.namespace);
    if let Some(state) = maps
        .get_opt(&format!("{}-installation", o.installation))
        .await?
    {
        ensure!(
            state.labels().get(LABEL) == Some(&o.installation)
                && state
                    .annotations()
                    .get("kimage.dev/manager")
                    .is_some_and(|m| m == "argocd"),
            "InstallationManagerConflict"
        );
        let (_, previous, release) =
            crate::lifecycle::state(client.clone(), &o.namespace, &o.installation).await?;
        // Identity, scope, storage and port remain fixed even during a transition.
        let mut requested = o.clone();
        if requested.registry_node.is_none() {
            requested.registry_node = previous.registry_node.clone();
        }
        if requested.worker_nodes.is_empty() {
            requested.worker_nodes = previous.worker_nodes.clone();
        }
        ensure!(
            serde_json::to_value(requested)? == serde_json::to_value(previous)?,
            "InstallationConfigurationChanged"
        );
        let pending =
            crate::lifecycle::journal(client.clone(), &o.namespace, &o.installation).await?;
        {
            let source = pending
                .as_ref()
                .filter(|j| j.phase != crate::lifecycle::Phase::Complete)
                .map_or(release.as_str(), |j| j.source.release.as_str());
            let rollback = crate::lifecycle::is_rollback(source, &desired.catalog.release)?;
            crate::lifecycle::transition(
                client.clone(),
                &o.namespace,
                &o.installation,
                &desired.catalog,
                false,
                rollback,
                None,
            )
            .await?;
        }
        // Apply explicit policy under the same lifecycle lock. The runtime
        // operator performs worker fencing and records the resulting placement.
        if desired.node_policy.is_some() || desired.build_policy.is_some() {
            let identity = kimage_core::oci::digest(serde_json::to_vec(desired)?.as_slice());
            let lease = crate::operation_lease::OperationLease::acquire_wait(
                client.clone(),
                &o.namespace,
                &o.installation,
                &format!("{}-lifecycle", o.installation),
                &identity[7..],
            )
            .await?;
            let changed=async {
                ensure!(!crate::lifecycle::active(client.clone(),&o.namespace,&o.installation).await?,"InstallationOperationPending");
                let context=crate::pull_install::apply::context(client.clone(),o,"")?;
                for (suffix,key,value) in [
                    ("node-policy","policy.json",desired.node_policy.as_ref().map(serde_json::to_string).transpose()?),
                    ("config","configuration.json",desired.build_policy.as_ref().map(|p|serde_json::json!({"schema":1,"buildPolicy":p}).to_string()))
                ] {
                    if let Some(value)=value {
                        lease.check().await?;
                        let resource=serde_json::json!({"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":format!("{}-{suffix}",o.installation),"namespace":o.namespace,"labels":{LABEL:o.installation},"annotations":{"kimage.dev/manager":"argocd"}},"data":{key:value}});
                        crate::lifecycle::replace_owned(&context,&resource,"argocd").await?;
                    }
                }
                Ok::<(),anyhow::Error>(())
            }.await;
            let released = lease.release().await;
            changed.and(released)?;
        }
    }
    let credentials = material(client.clone(), desired).await?;
    crate::pull_install::apply::install_managed(
        client.clone(),
        o,
        &desired.catalog,
        "argocd",
        Some(credentials),
        desired.node_policy.as_ref(),
        desired.build_policy.as_deref(),
    )
    .await?;
    let health = crate::diagnostics::inspect(client, &o.namespace, &o.installation, true).await?;
    ensure!(health["healthy"] == true, "GitOpsInstallationNotHealthy");
    Ok(())
}

/// The operator image also contains this privileged installation entry point.
/// Rendered manifests contain no private material or live Node snapshots.
pub async fn run(path: &Path) -> Result<()> {
    let client = crate::kubernetes::client().await?;
    let health = Arc::new(RwLock::new((false, String::from("Provisioning"))));
    let state = health.clone();
    let router = axum::Router::new()
        .route(
            "/livez",
            axum::routing::get(|| async { axum::http::StatusCode::OK }),
        )
        .route(
            "/readyz",
            axum::routing::get(move || {
                let state = state.clone();
                async move {
                    let (ready, reason) = state.read().unwrap().clone();
                    (
                        if ready {
                            axum::http::StatusCode::OK
                        } else {
                            axum::http::StatusCode::SERVICE_UNAVAILABLE
                        },
                        axum::Json(serde_json::json!({"ready":ready,"reason":reason})),
                    )
                }
            }),
        );
    let listener = crate::serving::listener("[::]:8080".parse()?)?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    let mut accepted_configuration = Vec::new();
    let stop = crate::serving::shutdown();
    tokio::pin!(stop);
    loop {
        let work = async {
            let bytes = tokio::fs::read(path).await?;
            ensure!(bytes.len() <= 256 * 1024, "GitOpsConfigurationTooLarge");
            if bytes != accepted_configuration {
                *health.write().unwrap() = (false, "Provisioning".into());
            }
            let desired: Desired = serde_json::from_slice(&bytes)?;
            reconcile(client.clone(), &desired).await?;
            accepted_configuration = bytes;
            Ok::<(), anyhow::Error>(())
        };
        let result = tokio::select! {_=&mut stop=>return Ok(()),r=work=>r};
        let (ready, reason) = match result {
            Ok(()) => (true, "Ready".to_owned()),
            Err(error) => {
                let text = error.to_string();
                let code = text.split(':').next().unwrap_or("");
                let safe = if !code.is_empty()
                    && code.len() <= 96
                    && code.bytes().all(|c| c.is_ascii_alphanumeric())
                {
                    code
                } else {
                    "GitOpsReconciliationPending"
                };
                tracing::warn!(
                    reason = safe,
                    "GitOps installation not ready; reconciliation will retry"
                );
                (false, safe.to_owned())
            }
        };
        *health.write().unwrap() = (ready, reason);
        tokio::select! {_=&mut stop=>return Ok(()),_=tokio::time::sleep(Duration::from_secs(15))=>{}}
    }
}

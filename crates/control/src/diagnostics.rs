//! Read-only installation diagnostics. JSON never includes credential material.
use anyhow::{Context, Result, ensure};
use k8s_openapi::api::core::v1::{ConfigMap, Pod, Secret};
use kube::{Api, Client, ResourceExt, api::ListParams};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio_rustls::rustls::{
    RootCertStore,
    client::{WebPkiServerVerifier, danger::ServerCertVerifier},
    pki_types::{ServerName, UnixTime},
};

fn tls_check(secret: &Secret, cert_key: &str, dns: &str, now: UnixTime) -> Result<()> {
    let data = secret.data.as_ref().context("MissingCertificateMaterial")?;
    let ca = &data.get("ca.crt").context("MissingCertificateAuthority")?.0;
    let cert = &data.get(cert_key).context("MissingCertificateLeaf")?.0;
    let mut roots = RootCertStore::empty();
    for cert in rustls_pemfile::certs(&mut ca.as_slice()) {
        roots.add(cert?)?;
    }
    let certificates =
        rustls_pemfile::certs(&mut cert.as_slice()).collect::<std::result::Result<Vec<_>, _>>()?;
    let (leaf, intermediates) = certificates
        .split_first()
        .context("MissingCertificateLeaf")?;
    if cert_key == "client.crt" {
        tokio_rustls::rustls::server::WebPkiClientVerifier::builder(Arc::new(roots))
            .build()?
            .verify_client_cert(leaf, intermediates, now)?;
        return Ok(());
    }
    let verifier = WebPkiServerVerifier::builder(Arc::new(roots)).build()?;
    verifier
        .verify_server_cert(
            leaf,
            intermediates,
            &ServerName::try_from(dns.to_owned())?,
            &[],
            now,
        )
        .context("CertificateInvalidOrExpired")?;
    Ok(())
}
pub async fn inspect(
    client: Client,
    namespace: &str,
    installation: &str,
    doctor: bool,
) -> Result<Value> {
    let (state, options, release) =
        crate::lifecycle::state(client.clone(), namespace, installation).await?;
    let manager = state
        .annotations()
        .get("kimage.dev/manager")
        .cloned()
        .unwrap_or_default();
    let mut issues = Vec::<Value>::new();
    let operation = crate::lifecycle::journal(client.clone(), namespace, installation).await?;
    if let Some(operation) = &operation
        && operation.phase != crate::lifecycle::Phase::Complete
    {
        issues.push(json!({"reason":"InstallationOperationPending","action":format!("Resume the {} operation with the same exact target {}",if crate::lifecycle::is_rollback(&operation.source.release,&operation.target.release)? {"rollback"}else{"upgrade"},operation.target.release)}));
    }
    let maps: Api<ConfigMap> = Api::namespaced(client.clone(), namespace);
    let policy = maps
        .get(&format!("{installation}-component-images"))
        .await?;
    crate::lifecycle::owned(&policy, installation)?;
    let catalog = crate::component_images::ImageCatalog::parse(
        policy
            .data
            .as_ref()
            .and_then(|d| d.get("images.json"))
            .context("MissingInstalledImageCatalog")?
            .as_bytes(),
        &release,
        false,
    )?;
    let nodes = crate::lifecycle::nodes(client.clone()).await?;
    let eligible = crate::pull_install::eligible_nodes(&nodes, options.runtime_profile);
    if eligible.is_err() {
        issues.push(json!({"reason":"NoCompatibleNodes","action":"Check Ready conditions, architecture labels and supported runtime versions"}));
    }
    let pods: Api<Pod> = Api::namespaced(client.clone(), namespace);
    let page = pods
        .list(
            &ListParams::default()
                .labels(&format!("{}={installation}", crate::install::LABEL))
                .limit(512),
        )
        .await?;
    ensure!(
        page.metadata
            .continue_
            .as_ref()
            .is_none_or(String::is_empty),
        "TooManyInstallationPods"
    );
    let components:Vec<_>=page.iter().map(|pod| {
        let ready=pod.status.as_ref().and_then(|s|s.conditions.as_ref()).is_some_and(|cs|cs.iter().any(|c|c.type_=="Ready"&&c.status=="True"));
        let completed=pod.status.as_ref().and_then(|s|s.phase.as_deref())==Some("Succeeded");
        let states:Vec<_>=pod.status.as_ref().and_then(|s|s.init_container_statuses.as_ref()).into_iter().flatten().chain(pod.status.as_ref().and_then(|s|s.container_statuses.as_ref()).into_iter().flatten()).map(|s|json!({"container":s.name,"ready":s.ready,"restarts":s.restart_count,"waiting":s.state.as_ref().and_then(|s|s.waiting.as_ref()).and_then(|w|w.reason.as_ref()),"terminated":s.state.as_ref().and_then(|s|s.terminated.as_ref()).map(|t|json!({"reason":t.reason,"exitCode":t.exit_code}))})).collect();
        if !ready && !completed {issues.push(json!({"reason":"ComponentNotReady","resource":format!("Pod/{}",pod.name_any()),"action":"Inspect Pod events and reported container waiting/termination reasons"}));}
        json!({"name":pod.name_any(),"uid":pod.uid(),"role":pod.labels().get("kimage.dev/role"),"node":pod.spec.as_ref().and_then(|s|s.node_name.as_ref()),"ready":ready,"phase":pod.status.as_ref().and_then(|s|s.phase.as_ref()),"containers":states,"images":pod.spec.as_ref().map(|s|s.containers.iter().map(|c|json!({"container":c.name,"image":c.image})).collect::<Vec<_>>())})
    }).collect();
    for role in ["registry", "operator", "admission"] {
        if !page
            .iter()
            .any(|p| p.labels().get("kimage.dev/role").is_some_and(|v| v == role))
        {
            issues.push(json!({"reason":"ComponentMissing","resource":role,"action":"Inspect the managed Deployment and its ReplicaSet events"}));
        }
    }
    let node_status = maps.get_opt(&format!("{installation}-node-status")).await?;
    if let Some(node_status) = node_status
        && crate::component_images::native_lifecycle(&release)?
    {
        crate::lifecycle::owned(&node_status, installation)?;
        let status: Value = serde_json::from_str(
            node_status
                .data
                .as_ref()
                .and_then(|d| d.get("status.json"))
                .context("MissingNodeReconciliationStatus")?,
        )?;
        if status["reason"] != "Ready"
            || status["checkedAt"]
                .as_i64()
                .is_none_or(|t| time::OffsetDateTime::now_utc().unix_timestamp() - t > 60)
        {
            issues.push(json!({"reason":"NodeReconciliationPending","detail":status["reason"],"action":"Inspect node readiness, node policy, worker leases and fencing; do not force-delete unreachable workers"}));
        }
    }
    let context = crate::pull_install::apply::context(
        client.clone(),
        &options,
        options.registry_node.as_deref().unwrap_or(""),
    )?;
    let claim = options
        .existing_claim
        .clone()
        .unwrap_or_else(|| format!("{installation}-data"));
    let pvc = context
        .api_at("PersistentVolumeClaim", Some(namespace))?
        .get(&claim)
        .await?;
    if pvc.data["status"]["phase"] != "Bound" {
        issues.push(json!({"reason":"RegistryStorageNotBound","action":"Inspect the retained PVC and its StorageClass; do not recreate the claim"}));
    }
    let mut certificates = Vec::new();
    if doctor {
        let secrets: Api<Secret> = Api::namespaced(client.clone(), namespace);
        for role in ["registry", "admission", "builder"] {
            let name = format!("{installation}-{role}-credentials");
            let secret = secrets.get(&name).await?;
            ensure!(
                secret
                    .labels()
                    .get(crate::install::LABEL)
                    .is_some_and(|v| v == installation),
                "CredentialOwnershipConflict"
            );
            let dns = format!("{installation}-{role}.{namespace}.svc");
            let key = if role == "builder" {
                "server.crt"
            } else {
                "tls.crt"
            };
            let now = UnixTime::now();
            let valid = tls_check(&secret, key, &dns, now).is_ok()
                && (role != "builder" || tls_check(&secret, "client.crt", &dns, now).is_ok());
            let future = UnixTime::since_unix_epoch(std::time::Duration::from_secs(
                now.as_secs().saturating_add(30 * 86400),
            ));
            let valid_thirty_days = tls_check(&secret, key, &dns, future).is_ok()
                && (role != "builder" || tls_check(&secret, "client.crt", &dns, future).is_ok());
            certificates
                .push(json!({"role":role,"validNow":valid,"validForThirtyDays":valid_thirty_days}));
            if !valid || !valid_thirty_days {
                issues.push(json!({"reason":"CertificateInvalidOrExpiring","resource":format!("Secret/{name}"),"action":"Rotate the affected leaf credentials before expiry; preserve the installation identity"}));
            }
        }
        let credentials = crate::pull_install::credentials::Credentials::resolve(
            client.clone(),
            namespace,
            installation,
        )
        .await?;
        let policy = crate::node_reconcile::policy(client.clone(), namespace, installation).await?;
        let planned = crate::node_reconcile::select(&options, &policy, &nodes).and_then(
            |(nodes, workers)| {
                let mut options = options.clone();
                options.worker_nodes = workers;
                crate::pull_install::Plan::with_manager(
                    &options,
                    &catalog,
                    &nodes,
                    &credentials,
                    &manager,
                )
            },
        );
        match planned {
            Ok(mut plan) => {
                if crate::component_images::native_lifecycle(&release)? {crate::node_reconcile::decorate(&mut plan.resources,&state)?;}
                for desired in plan.resources.iter().chain(&plan.webhooks) {
                    let kind=desired["kind"].as_str().unwrap();
                    let name=desired["metadata"]["name"].as_str().unwrap();
                    if kind=="ConfigMap" && (name==format!("{installation}-config") || name==format!("{installation}-node-policy")){continue;}
                    let ns=crate::pull_install::apply::namespaced(kind).then(||desired["metadata"]["namespace"].as_str().unwrap());
                    let current=context.api_at(kind,ns)?.get_opt(name).await?;
                    let matches=current.as_ref().is_some_and(|object| {
                        object.labels().get(crate::install::LABEL).is_some_and(|v|v==installation) && (
                            crate::install::verify_declared(desired,object).is_ok() ||
                            kind=="ConfigMap" && name.starts_with(&format!("{installation}-builder-template-")) && crate::node_reconcile::equivalent_template(desired,&serde_json::to_value(object).unwrap_or_default())
                        )
                    });
                    if !matches {issues.push(json!({"reason":"InstallationResourceDrift","resource":format!("{kind}/{name}"),"action":"Compare the resource with the installation's pinned release before repairing it"}));}
                }
            },
            Err(_)=>issues.push(json!({"reason":"NativePlacementUnavailable","action":"Check registry node identity and configured native workers; registry storage is never moved automatically"})),
        }
    }
    let platforms: std::collections::BTreeSet<_> = page
        .iter()
        .filter(|p| {
            p.labels()
                .get("kimage.dev/role")
                .is_some_and(|r| r == "builder")
                && p.status
                    .as_ref()
                    .and_then(|s| s.conditions.as_ref())
                    .is_some_and(|cs| cs.iter().any(|c| c.type_ == "Ready" && c.status == "True"))
        })
        .filter_map(|p| p.labels().get("kimage.dev/worker-architecture").cloned())
        .collect();
    Ok(
        json!({"schema":1,"installation":installation,"namespace":namespace,"uid":state.uid(),"release":release,"manager":manager,"operation":operation.as_ref().map(|j|json!({"source":j.source.release,"target":j.target.release,"phase":j.phase})),"platforms":platforms,"storage":{"claim":claim,"uid":pvc.uid(),"volume":pvc.data["spec"]["volumeName"],"phase":pvc.data["status"]["phase"]},"components":components,"certificates":certificates,"healthy":issues.is_empty(),"issues":issues}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine, engine::general_purpose::STANDARD};
    #[test]
    fn checks_server_and_client_certificate_expiry_and_canonical_names() {
        let material =
            crate::pull_install::credentials::Credentials::generate("system", "dev").unwrap();
        let secret = |data: crate::pull_install::credentials::Material| Secret {
            data: Some(
                data.into_iter()
                    .map(|(k, v)| (k, k8s_openapi::ByteString(STANDARD.decode(v).unwrap())))
                    .collect(),
            ),
            ..Default::default()
        };
        let registry = secret(material.registry);
        let now = UnixTime::now();
        tls_check(&registry, "tls.crt", "dev-registry.system.svc", now).unwrap();
        assert!(tls_check(&registry, "tls.crt", "other.system.svc", now).is_err());
        let expired = UnixTime::since_unix_epoch(std::time::Duration::from_secs(
            now.as_secs() + 3660 * 86400,
        ));
        assert!(tls_check(&registry, "tls.crt", "dev-registry.system.svc", expired).is_err());
        let builder = secret(material.builder);
        tls_check(&builder, "client.crt", "unused", now).unwrap();
        assert!(tls_check(&builder, "client.crt", "unused", expired).is_err());
    }
}

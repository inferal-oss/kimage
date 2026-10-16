//! Apply small Kubernetes resources. Image layers never pass through this path.
use super::{Options, Plan, credentials::Credentials};
use crate::{
    component_images::ImageCatalog,
    install::{Installer, LABEL},
};
use anyhow::{Context, Result, ensure};
use k8s_openapi::api::core::v1::{ConfigMap, Node};
use kube::{Api, Client, ResourceExt, api::ListParams};
use serde_json::{Value, json};
use std::{path::PathBuf, time::Duration};

pub(crate) fn context(client: Client, options: &Options, primary: &str) -> Result<Installer> {
    Installer::image_pull_context(
        client,
        crate::install::Options {
            scope: options.scope,
            excluded_namespaces: options.excluded_namespaces.clone(),
            application_namespace: options.application_namespace.clone(),
            runtime_profile: options.runtime_profile,
            namespace: options.namespace.clone(),
            installation: options.installation.clone(),
            node: primary.into(),
            carrier: String::new(),
            storage_class: options.storage_class.clone().unwrap_or_default(),
            pvc: options.existing_claim.clone(),
            port: options.port,
            credentials: PathBuf::new(),
        },
    )
}
pub(crate) fn namespaced(kind: &str) -> bool {
    !matches!(
        kind,
        "ClusterRole"
            | "ClusterRoleBinding"
            | "MutatingWebhookConfiguration"
            | "ValidatingWebhookConfiguration"
    )
}
fn resume_options(
    options: &Options,
    nodes: &[Node],
    state: Option<&k8s_openapi::api::core::v1::ConfigMap>,
    manager: &str,
) -> Result<Options> {
    let mut resolved = options.clone();
    if let Some(state) = state {
        ensure!(
            state
                .labels()
                .get(LABEL)
                .is_some_and(|label| label == &options.installation)
                && state
                    .annotations()
                    .get("kimage.dev/manager")
                    .is_some_and(|existing| existing == manager),
            "InstallationManagerConflict"
        );
        let record: Value = serde_json::from_str(
            state
                .data
                .as_ref()
                .and_then(|data| data.get("installation.json"))
                .context("MissingInstallationState")?,
        )?;
        ensure!(
            record["schema"] == 1 && record["delivery"] == "ImagePull",
            "UnsupportedInstallationState"
        );
        let previous: Options = serde_json::from_value(record["options"].clone())?;
        ensure!(
            previous.namespace == options.namespace
                && previous.installation == options.installation,
            "InstallationStateIdentityMismatch"
        );
        if resolved.registry_node.is_none() {
            resolved.registry_node = previous.registry_node.clone();
        }
        ensure!(
            resolved.registry_node == previous.registry_node,
            "RegistryPlacementChangeRequiresStorageRecovery"
        );
        if resolved.worker_nodes.is_empty() {
            resolved.worker_nodes = previous.worker_nodes.clone();
        }
        ensure!(
            serde_json::to_value(&resolved)? == serde_json::to_value(&previous)?,
            "InstallationConfigurationChanged: preserve the existing scope, storage, port and placement"
        );
        let node = nodes
            .iter()
            .find(|node| Some(node.name_any()) == resolved.registry_node)
            .context("RetainedRegistryNodeUnavailable")?;
        ensure!(
            record["registryNodeUid"].as_str() == node.uid().as_deref(),
            "RegistryNodeIdentityChanged: recover its retained storage before replacing the node"
        );
    }
    Ok(resolved)
}
fn build_configuration(map: Option<&ConfigMap>, installation: &str) -> Result<String> {
    if let Some(map) = map {
        crate::configuration::parse(map, installation)?;
        return Ok(map
            .data
            .as_ref()
            .and_then(|data| data.get("configuration.json"))
            .context("MissingInstallationConfiguration")?
            .clone());
    }
    Ok(serde_json::to_string(
        &crate::configuration::Configuration::default(),
    )?)
}
async fn apply_one(context: &Installer, resource: &Value, manager: &str) -> Result<()> {
    let kind = resource["kind"]
        .as_str()
        .context("MissingInstallResourceKind")?;
    let ns = namespaced(kind).then(|| resource["metadata"]["namespace"].as_str().unwrap());
    let api = context.api_at(kind, ns)?;
    let name = resource["metadata"]["name"]
        .as_str()
        .context("MissingInstallResourceName")?;
    if let Some(existing) = api.get_opt(name).await? {
        ensure!(
            existing
                .annotations()
                .get("kimage.dev/manager")
                .is_some_and(|existing| existing == manager)
                && !existing
                    .annotations()
                    .contains_key("meta.helm.sh/release-name"),
            "InstallationManagerConflict: {kind}/{name}; use the manager that owns this installation"
        );
    }
    if manager == "argocd"
        && kind == "ConfigMap"
        && [
            format!("{}-config", context.options.installation),
            format!("{}-node-policy", context.options.installation),
        ]
        .contains(&name.to_string())
    {
        return crate::lifecycle::replace_owned(context, resource, manager).await;
    }
    context.owned_at(resource.clone(), ns).await
}
pub async fn install(client: Client, options: &Options, catalog: &ImageCatalog) -> Result<()> {
    install_managed(client, options, catalog, "cli", None, None, None).await
}

pub(crate) async fn install_managed(
    client: Client,
    options: &Options,
    catalog: &ImageCatalog,
    manager: &str,
    supplied: Option<Credentials>,
    requested_policy: Option<&crate::node_reconcile::Policy>,
    requested_build_policy: Option<&str>,
) -> Result<()> {
    ensure!(
        matches!(manager, "cli" | "argocd"),
        "InvalidInstallationManager"
    );
    options.validate()?;
    catalog.validate(&catalog.release, false)?;
    let node_api: Api<Node> = Api::all(client.clone());
    let page = node_api.list(&ListParams::default().limit(1024)).await?;
    ensure!(
        page.metadata
            .continue_
            .as_ref()
            .is_none_or(String::is_empty),
        "TooManyClusterNodes"
    );
    let maps: Api<k8s_openapi::api::core::v1::ConfigMap> =
        Api::namespaced(client.clone(), &options.namespace);
    let previous = maps
        .get_opt(&format!("{}-installation", options.installation))
        .await?;
    if let Some(previous) = &previous {
        let record: Value = serde_json::from_str(
            previous
                .data
                .as_ref()
                .and_then(|data| data.get("installation.json"))
                .context("MissingInstallationState")?,
        )?;
        ensure!(
            record["release"] == catalog.release,
            "ImplicitReleaseUpgradeRefused: use a compatible release manager"
        );
    }
    if previous.is_some() {
        ensure!(
            !crate::lifecycle::active(client.clone(), &options.namespace, &options.installation)
                .await?,
            "InstallationOperationPending: resume the explicit upgrade or rollback"
        );
    }
    let mut resolved = resume_options(options, &page.items, previous.as_ref(), manager)?;
    let node_policy = match requested_policy {
        Some(policy) => policy.clone(),
        None => {
            crate::node_reconcile::policy(client.clone(), &options.namespace, &options.installation)
                .await?
        }
    };
    let (selected, workers) = crate::node_reconcile::select(&resolved, &node_policy, &page.items)?;
    resolved.worker_nodes = workers;
    let options = &resolved;
    let credentials = match supplied {
        Some(credentials) => credentials,
        None => {
            Credentials::resolve(client.clone(), &options.namespace, &options.installation).await?
        }
    };
    let mut plan = Plan::with_manager(options, catalog, &selected, &credentials, manager)?;
    let context = context(client.clone(), options, &plan.primary_node)?;
    context.ensure_scope_available().await?;
    let maps = context.api_at("ConfigMap", Some(&options.namespace))?;
    ensure!(
        maps.get_opt(&format!("{}-state", options.installation))
            .await?
            .is_none(),
        "LegacyInstallationDetected: use its retained installer; automatic bootstrap migration is not supported"
    );
    // Read-only compatibility checks happen before creating any namespace,
    // storage, credential, or serving object.
    let crds: Api<k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition>=Api::all(client.clone());
    for desired in crate::schema::definitions()? {
        if let Some(existing) = crds.get_opt(&desired.name_any()).await? {
            crate::schema::compatibility(&existing, &desired)?;
        }
    }
    // Reapplying an installation must preserve an administrator's build policy.
    let configuration_maps: Api<ConfigMap> = Api::namespaced(client.clone(), &options.namespace);
    let previous_configuration = configuration_maps
        .get_opt(&format!("{}-config", options.installation))
        .await?;
    let configuration = match requested_build_policy {
        Some(policy) => {
            ensure!(matches!(policy, "OnDemand" | "Eager"), "InvalidBuildPolicy");
            json!({"schema":1,"buildPolicy":policy}).to_string()
        }
        None => build_configuration(previous_configuration.as_ref(), &options.installation)?,
    };
    for resource in &mut plan.resources {
        if resource["kind"] == "ConfigMap"
            && resource["metadata"]["name"] == format!("{}-node-policy", options.installation)
        {
            resource["data"]["policy.json"] = json!(serde_json::to_string(&node_policy)?);
        }
        if resource["kind"] == "ConfigMap"
            && resource["metadata"]["name"] == format!("{}-config", options.installation)
        {
            resource["data"]["configuration.json"] = json!(configuration);
        }
    }
    if requested_policy.is_none()
        && let Some(policy) = configuration_maps
            .get_opt(&format!("{}-node-policy", options.installation))
            .await?
    {
        crate::lifecycle::owned(&policy, &options.installation)?;
        for resource in &mut plan.resources {
            if resource["kind"] == "ConfigMap" && resource["metadata"]["name"] == policy.name_any()
            {
                resource["data"]["policy.json"] = json!(
                    policy
                        .data
                        .as_ref()
                        .and_then(|d| d.get("policy.json"))
                        .context("MissingNodePolicy")?
                );
            }
        }
    }
    if crate::component_images::native_lifecycle(&catalog.release)?
        && let Some(state) = &previous
    {
        crate::node_reconcile::decorate(&mut plan.resources, state)?;
        for resource in &mut plan.resources {
            if resource["kind"] == "ConfigMap"
                && resource["metadata"]["name"].as_str().is_some_and(|n| {
                    n.starts_with(&format!("{}-builder-template-", options.installation))
                })
                && let Some(existing) = configuration_maps
                    .get_opt(resource["metadata"]["name"].as_str().unwrap())
                    .await?
            {
                let actual = serde_json::to_value(&existing)?;
                if crate::node_reconcile::equivalent_template(resource, &actual) {
                    resource["data"]["pod.json"] = actual["data"]["pod.json"].clone();
                }
            }
        }
    }
    context.ensure_namespaces().await?;
    if let Some(claim) = &options.existing_claim {
        let pvc = context
            .api_at("PersistentVolumeClaim", Some(&options.namespace))?
            .get(claim)
            .await?;
        ensure!(
            pvc.data["status"]["phase"] == "Bound",
            "ExistingPvcNotBound"
        );
    }
    let identity = kimage_core::oci::digest(
        format!("install:{}:{}", options.namespace, options.installation).as_bytes(),
    );
    let lease = crate::operation_lease::OperationLease::acquire_wait(
        client.clone(),
        &options.namespace,
        &options.installation,
        &format!("{}-lifecycle", options.installation),
        &identity[7..],
    )
    .await?;
    let applied = async {
        for resource in &plan.resources {
            lease.check().await?;
            apply_one(&context, resource, manager).await?;
        }
        wait_ready(&context, &plan).await?;
        for resource in &plan.webhooks {
            lease.check().await?;
            apply_one(&context, resource, manager).await?;
        }
        Ok::<(), anyhow::Error>(())
    }
    .await;
    let released = lease.release().await;
    applied?;
    released?;
    tracing::info!(installation=%options.installation,namespace=%options.namespace,release=%catalog.release,"registry-pulled installation ready");
    Ok(())
}
pub(crate) async fn wait_ready(context: &Installer, plan: &Plan) -> Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(600);
    loop {
        let mut pending = Vec::new();
        for resource in &plan.resources {
            let kind = resource["kind"].as_str().unwrap();
            if !matches!(kind, "Pod" | "Deployment" | "Job") {
                continue;
            }
            let name = resource["metadata"]["name"].as_str().unwrap();
            let current = context
                .api_at(kind, Some(&context.options.namespace))?
                .get_opt(name)
                .await?;
            let Some(current) = current else {
                pending.push(format!("{kind}/{name}"));
                continue;
            };
            ensure!(
                current.labels().get(LABEL) == Some(&context.options.installation),
                "ComponentReadinessOwnershipConflict: {kind}/{name}"
            );
            crate::install::verify_declared(resource, &current)?;
            if current.metadata.deletion_timestamp.is_some() {
                pending.push(format!("{kind}/{name}"));
                continue;
            }
            let status = &current.data["status"];
            let ready = match kind {
                "Pod" => status["conditions"].as_array().is_some_and(|cs| {
                    cs.iter()
                        .any(|c| c["type"] == "Ready" && c["status"] == "True")
                }),
                "Deployment" => {
                    status["observedGeneration"].as_i64().unwrap_or(0)
                        >= current.metadata.generation.unwrap_or(1)
                        && status["availableReplicas"] == 1
                        && status["updatedReplicas"] == 1
                }
                "Job" => status["succeeded"].as_u64().is_some_and(|n| n >= 1),
                _ => unreachable!(),
            };
            ensure!(
                !(kind == "Job"
                    && status["conditions"].as_array().is_some_and(|cs| cs
                        .iter()
                        .any(|c| c["type"] == "Failed" && c["status"] == "True"))),
                "ComponentAuthenticatedReadinessFailed: {name}"
            );
            if !ready {
                pending.push(format!("{kind}/{name}"));
            }
        }
        if pending.is_empty() {
            return Ok(());
        }
        ensure!(
            tokio::time::Instant::now() < deadline,
            "ImagePullInstallationNotReady: {}; inspect Pod events, image pulls, and supported runtime requirements",
            pending.join(", ")
        );
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

pub async fn uninstall(client: Client, options: &Options) -> Result<()> {
    options.validate()?;
    let context = context(
        client,
        options,
        options.registry_node.as_deref().unwrap_or(""),
    )?;
    let state = context
        .api_at("ConfigMap", Some(&options.namespace))?
        .get(&format!("{}-installation", options.installation))
        .await?;
    ensure!(
        state
            .labels()
            .get(LABEL)
            .is_some_and(|label| label == &options.installation)
            && state
                .annotations()
                .get("kimage.dev/manager")
                .is_some_and(|manager| matches!(manager.as_str(), "cli" | "argocd")),
        "InstallationManagerConflict: use helm uninstall for a Helm-managed installation"
    );
    if state
        .annotations()
        .get("kimage.dev/manager")
        .is_some_and(|m| m == "argocd")
    {
        ensure!(
            context
                .api_at("Deployment", Some(&options.namespace))?
                .get_opt(&format!("{}-provisioner", options.installation))
                .await?
                .is_none(),
            "ArgoCDProvisionerActive: remove the Application before uninstalling its retained runtime"
        );
    }
    // Foreground deletion fences supervisors before deleting individual workers
    // and their active build sessions. Their PVC and release catalog remain.
    for kind in ["Deployment", "Job"] {
        let api = context.api_at(kind, Some(&options.namespace))?;
        for object in api
            .list(&ListParams::default().labels(&format!("{LABEL}={}", options.installation)))
            .await?
        {
            ensure!(
                object
                    .annotations()
                    .get("kimage.dev/manager")
                    .is_some_and(
                        |manager| state.annotations().get("kimage.dev/manager") == Some(manager)
                    ),
                "InstallationManagerConflict"
            );
            crate::install::delete_foreground_snapshot(&api, &object).await?;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
            while api.get_opt(&object.name_any()).await?.is_some() {
                ensure!(
                    tokio::time::Instant::now() < deadline,
                    "ComponentDeletionPending"
                );
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
    context.uninstall().await
}

pub async fn uninstall_installed(
    client: Client,
    namespace: &str,
    installation: &str,
) -> Result<()> {
    let maps: Api<k8s_openapi::api::core::v1::ConfigMap> =
        Api::namespaced(client.clone(), namespace);
    let state = maps.get(&format!("{installation}-installation")).await?;
    ensure!(
        state
            .labels()
            .get(LABEL)
            .is_some_and(|label| label == installation)
            && state
                .annotations()
                .get("kimage.dev/manager")
                .is_some_and(|manager| matches!(manager.as_str(), "cli" | "argocd")),
        "InstallationManagerConflict"
    );
    let configuration: Value = serde_json::from_str(
        state
            .data
            .as_ref()
            .and_then(|data| data.get("installation.json"))
            .context("MissingInstallationState")?,
    )?;
    ensure!(
        configuration["schema"] == 1 && configuration["delivery"] == "ImagePull",
        "UnsupportedInstallationState"
    );
    let options: Options = serde_json::from_value(configuration["options"].clone())?;
    ensure!(
        options.namespace == namespace && options.installation == installation,
        "InstallationStateIdentityMismatch"
    );
    ensure!(
        !crate::lifecycle::active(client.clone(), namespace, installation).await?,
        "InstallationOperationPending: resume its exact target before uninstall"
    );
    let identity =
        kimage_core::oci::digest(format!("uninstall:{namespace}:{installation}").as_bytes());
    let lease = crate::operation_lease::OperationLease::acquire_wait(
        client.clone(),
        namespace,
        installation,
        &format!("{installation}-lifecycle"),
        &identity[7..],
    )
    .await?;
    let operation = async {
        let latest = maps.get(&format!("{installation}-installation")).await?;
        ensure!(
            latest.uid() == state.uid(),
            "InstallationChangedDuringUninstallPreflight"
        );
        ensure!(
            !crate::lifecycle::active(client.clone(), namespace, installation).await?,
            "InstallationOperationPending: resume its exact target before uninstall"
        );
        uninstall(client, &options).await
    };
    let result = tokio::select! {
        result = operation => result,
        _ = lease.lost.cancelled() => Err(anyhow::anyhow!("OperationLeaseLost")),
    };
    let released = lease.release().await;
    result.and(released)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn options() -> Options {
        serde_json::from_value(json!({"namespace":"kimage-system","installation":"kimage","scope":"namespace","applicationNamespace":"apps","excludedNamespaces":[],"runtimeProfile":"inferal","registryNode":null,"storageClass":null,"storageSize":"1Gi","existingClaim":null,"port":17701})).unwrap()
    }
    fn node(name: &str, uid: &str) -> Node {
        serde_json::from_value(json!({"metadata":{"name":name,"uid":uid}})).unwrap()
    }
    #[test]
    fn reapply_preserves_valid_administrator_configuration_bytes() {
        let raw = r#"{
  "buildPolicy": "Eager", "schema": 1
}"#;
        let mut map: ConfigMap = serde_json::from_value(
            json!({"metadata":{"labels":{LABEL:"kimage"}},"data":{"configuration.json":raw}}),
        )
        .unwrap();
        assert_eq!(build_configuration(Some(&map), "kimage").unwrap(), raw);
        map.data
            .as_mut()
            .unwrap()
            .insert("configuration.json".into(), r#"{"schema":99}"#.into());
        assert!(build_configuration(Some(&map), "kimage").is_err());
    }
    #[test]
    fn reapply_retains_storage_and_worker_placement_and_refuses_identity_or_config_changes() {
        let requested = options();
        let mut previous = requested.clone();
        previous.registry_node = Some("original".into());
        previous
            .worker_nodes
            .insert("amd64".into(), "original".into());
        let mut state: ConfigMap = serde_json::from_value(json!({
            "metadata":{"labels":{LABEL:"kimage"},"annotations":{"kimage.dev/manager":"cli"}},
            "data":{"installation.json":serde_json::to_string(&json!({"schema":1,"delivery":"ImagePull","options":previous,"registryNodeUid":"original-uid","release":"0.4.0"})).unwrap()}
        })).unwrap();
        let nodes = [
            node("added-first", "added-uid"),
            node("original", "original-uid"),
        ];
        let resolved = resume_options(&requested, &nodes, Some(&state), "cli").unwrap();
        assert_eq!(resolved.registry_node.as_deref(), Some("original"));
        assert_eq!(resolved.worker_nodes["amd64"], "original");
        assert!(
            resume_options(
                &requested,
                &[node("original", "replacement-uid")],
                Some(&state),
                "cli"
            )
            .is_err()
        );
        let mut changed = requested.clone();
        changed.registry_node = Some("added-first".into());
        assert!(resume_options(&changed, &nodes, Some(&state), "cli").is_err());
        changed = requested.clone();
        changed.application_namespace = Some("other".into());
        assert!(resume_options(&changed, &nodes, Some(&state), "cli").is_err());
        changed = requested.clone();
        changed.port += 1;
        assert!(resume_options(&changed, &nodes, Some(&state), "cli").is_err());
        state
            .metadata
            .annotations
            .as_mut()
            .unwrap()
            .insert("kimage.dev/manager".into(), "helm".into());
        assert!(resume_options(&requested, &nodes, Some(&state), "cli").is_err());
    }
}

//! UID-bound node membership and native workers, serialized with release changes.
use crate::{
    install::LABEL,
    lifecycle,
    operation_lease::OperationLease,
    pull_install::{self, Options},
};
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use k8s_openapi::api::core::v1::{ConfigMap, Node, Pod, Secret};
use kube::{
    Api, Client, ResourceExt,
    api::{DynamicObject, ListParams, PostParams},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Policy {
    pub schema: u32,
    #[serde(default)]
    pub node_selector: BTreeMap<String, String>,
    #[serde(default)]
    pub worker_pins: BTreeMap<String, String>,
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            schema: 1,
            node_selector: BTreeMap::new(),
            worker_pins: BTreeMap::new(),
        }
    }
}
pub async fn policy(client: Client, namespace: &str, id: &str) -> Result<Policy> {
    let map = Api::<ConfigMap>::namespaced(client, namespace)
        .get_opt(&format!("{id}-node-policy"))
        .await?;
    let Some(map) = map else {
        return Ok(Policy::default());
    };
    lifecycle::owned(&map, id)?;
    let result: Policy = serde_json::from_str(
        map.data
            .as_ref()
            .and_then(|d| d.get("policy.json"))
            .context("MissingNodePolicy")?,
    )?;
    ensure!(
        result.schema == 1
            && result.node_selector.len() <= 32
            && result
                .node_selector
                .iter()
                .all(|(k, v)| !k.is_empty() && k.len() <= 253 && v.len() <= 63)
            && result.worker_pins.len() <= 2
            && result
                .worker_pins
                .keys()
                .all(|a| matches!(a.as_str(), "amd64" | "arm64")),
        "InvalidNodePolicy"
    );
    Ok(result)
}
pub fn select(
    options: &Options,
    policy: &Policy,
    nodes: &[Node],
) -> Result<(Vec<Node>, BTreeMap<String, String>)> {
    let candidates: Vec<_> = nodes
        .iter()
        .filter(|n| {
            policy
                .node_selector
                .iter()
                .all(|(k, v)| n.labels().get(k) == Some(v))
        })
        .cloned()
        .collect();
    let selected = pull_install::eligible_nodes(&candidates, options.runtime_profile)?;
    ensure!(!selected.is_empty(), "NoEligibleInstallationNodes");
    ensure!(
        options
            .registry_node
            .as_ref()
            .is_none_or(|name| selected.iter().any(|n| n.name_any() == *name)),
        "RegistryNodeNotEligible: registry storage is never moved automatically"
    );
    let mut workers = BTreeMap::new();
    for node in &selected {
        let arch = node
            .labels()
            .get("kubernetes.io/arch")
            .context("MissingNodeArchitecture")?;
        workers
            .entry(arch.clone())
            .or_insert_with(|| node.name_any());
    }
    for (arch, node) in &options.worker_nodes {
        if selected
            .iter()
            .any(|n| n.name_any() == *node && n.labels().get("kubernetes.io/arch") == Some(arch))
        {
            workers.insert(arch.clone(), node.clone());
        }
    }
    for (arch, node) in &policy.worker_pins {
        ensure!(
            selected.iter().any(
                |n| n.name_any() == *node && n.labels().get("kubernetes.io/arch") == Some(arch)
            ),
            "PinnedWorkerUnavailable: administrator placement requires attention"
        );
        workers.insert(arch.clone(), node.clone());
    }
    Ok((selected, workers))
}
fn ready(pod: &DynamicObject) -> bool {
    pod.metadata.deletion_timestamp.is_none()
        && pod.data["status"]["conditions"]
            .as_array()
            .is_some_and(|cs| {
                cs.iter()
                    .any(|c| c["type"] == "Ready" && c["status"] == "True")
            })
}
fn decorate_metadata(metadata: &mut Value, state: &ConfigMap) -> Result<()> {
    metadata["ownerReferences"] = json!([{"apiVersion":"v1","kind":"ConfigMap","name":state.name_any(),"uid":state.uid().context("MissingInstallationUid")?,"controller":false,"blockOwnerDeletion":false}]);
    if state
        .annotations()
        .get("kimage.dev/manager")
        .is_some_and(|v| v == "helm")
    {
        metadata["labels"]["app.kubernetes.io/managed-by"] = json!("Helm");
        for key in [
            "meta.helm.sh/release-name",
            "meta.helm.sh/release-namespace",
        ] {
            metadata["annotations"][key] = json!(
                state
                    .annotations()
                    .get(key)
                    .context("MissingHelmReleaseIdentity")?
            );
        }
    }
    Ok(())
}
pub(crate) fn decorate(resources: &mut [Value], state: &ConfigMap) -> Result<()> {
    let id = state
        .labels()
        .get(LABEL)
        .context("MissingInstallationIdentity")?;
    for resource in resources {
        let name = resource["metadata"]["name"]
            .as_str()
            .context("MissingResourceName")?
            .to_owned();
        let native = name.starts_with(&format!("{id}-n-"))
            || name.starts_with(&format!("{id}-builders-"))
            || (name.starts_with(&format!("{id}-builder-"))
                && name != format!("{id}-builder-credentials"));
        if !native {
            continue;
        }
        decorate_metadata(&mut resource["metadata"], state)?;
        if resource["kind"] == "ConfigMap" && name.starts_with(&format!("{id}-builder-template-")) {
            let mut pod: Value = serde_json::from_str(
                resource["data"]["pod.json"]
                    .as_str()
                    .context("MissingBuilderTemplate")?,
            )?;
            decorate_metadata(&mut pod["metadata"], state)?;
            resource["data"]["pod.json"] = json!(serde_json::to_string(&pod)?);
        }
    }
    Ok(())
}
/// Runtime adds only installation-bound ownership to canonical recovery Pods.
/// Accept both the initial static chart template and that decorated equivalent.
pub(crate) fn equivalent_template(expected: &Value, actual: &Value) -> bool {
    let parse = |value: &Value| {
        value["data"]["pod.json"]
            .as_str()
            .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
    };
    let (Some(mut expected), Some(mut actual)) = (parse(expected), parse(actual)) else {
        return false;
    };
    if !actual["metadata"]["ownerReferences"].is_null()
        && actual["metadata"]["ownerReferences"] != expected["metadata"]["ownerReferences"]
    {
        return false;
    }
    for key in [
        "meta.helm.sh/release-name",
        "meta.helm.sh/release-namespace",
    ] {
        if !actual["metadata"]["annotations"][key].is_null()
            && actual["metadata"]["annotations"][key] != expected["metadata"]["annotations"][key]
        {
            return false;
        }
        for pod in [&mut expected, &mut actual] {
            if let Some(annotations) = pod["metadata"]["annotations"].as_object_mut() {
                annotations.remove(key);
            }
        }
    }
    if !actual["metadata"]["labels"]["app.kubernetes.io/managed-by"].is_null()
        && actual["metadata"]["labels"]["app.kubernetes.io/managed-by"]
            != expected["metadata"]["labels"]["app.kubernetes.io/managed-by"]
    {
        return false;
    }
    for pod in [&mut expected, &mut actual] {
        pod["metadata"]
            .as_object_mut()
            .unwrap()
            .remove("ownerReferences");
        if let Some(labels) = pod["metadata"]["labels"].as_object_mut() {
            labels.remove("app.kubernetes.io/managed-by");
        }
    }
    expected == actual
}
pub async fn once(config: Arc<crate::reconcile::Config>) -> Result<()> {
    let Some(settings) = &config.direct else {
        return Ok(());
    };
    let ns = &settings.system_namespace;
    let id = &settings.installation;
    let maps: Api<ConfigMap> = Api::namespaced(config.client.clone(), ns);
    if maps.get_opt(&format!("{id}-installation")).await?.is_none() {
        return Ok(());
    }
    if lifecycle::active(config.client.clone(), ns, id).await? {
        return Ok(());
    }
    let identity = kimage_core::oci::digest(format!("node-reconcile:{ns}:{id}").as_bytes());
    let lease = match OperationLease::acquire(
        config.client.clone(),
        ns,
        id,
        &format!("{id}-lifecycle"),
        &identity[7..],
    )
    .await
    {
        Ok(v) => v,
        Err(e) if e.to_string() == "OperationBusy" => return Ok(()),
        Err(e) => return Err(e),
    };
    let result = async {
        if lifecycle::active(config.client.clone(), ns, id).await? {
            return Ok(());
        }
        let (state, mut options, release) = lifecycle::state(config.client.clone(), ns, id).await?;
        ensure!(
            release == env!("CARGO_PKG_VERSION"),
            "InstalledOperatorVersionMismatch"
        );
        let manager = state
            .annotations()
            .get("kimage.dev/manager")
            .context("MissingInstallationManager")?;
        let all = lifecycle::nodes(config.client.clone()).await?;
        let policy = policy(config.client.clone(), ns, id).await?;
        let (nodes, workers) = select(&options, &policy, &all)?;
        let state_record: Value = serde_json::from_str(
            &state.data.as_ref().context("MissingInstallationState")?["installation.json"],
        )?;
        let primary = nodes
            .iter()
            .find(|n| Some(n.name_any()) == options.registry_node)
            .context("RegistryNodeNotEligible")?;
        ensure!(
            state_record["registryNodeUid"].as_str() == primary.uid().as_deref(),
            "RegistryNodeIdentityChanged"
        );
        let catalog =
            crate::component_images::installed_policy(config.client.clone(), ns, id).await?;
        let secrets: Api<Secret> = Api::namespaced(config.client.clone(), ns);
        let credentials = secrets.get(&format!("{id}-builder-credentials")).await?;
        ensure!(
            credentials.labels().get(LABEL) == Some(id),
            "BuilderCredentialOwnershipConflict"
        );
        let material = credentials
            .data
            .context("MissingBuilderCredential")?
            .into_iter()
            .map(|(k, v)| (k, STANDARD.encode(v.0)))
            .collect();
        let mut resources = pull_install::native_resources(
            &options, &catalog, &nodes, &workers, &material, manager,
        )?;
        decorate(&mut resources, &state)?;
        let context = pull_install::apply::context(
            config.client.clone(),
            &options,
            options.registry_node.as_deref().unwrap(),
        )?;
        let pods = context.api_at("Pod", Some(ns))?;
        let cm = context.api_at("ConfigMap", Some(ns))?;
        // Revocation precedes deletion of the corresponding UID-bound proxy.
        for object in cm
            .list(
                &ListParams::default()
                    .labels(&format!("{LABEL}={id},kimage.dev/role=node-endpoint")),
            )
            .await?
        {
            let wanted = resources
                .iter()
                .any(|v| v["kind"] == "ConfigMap" && v["metadata"]["name"] == object.name_any());
            if !wanted {
                lease.check().await?;
                ensure!(
                    object.annotations().get("kimage.dev/manager") == Some(manager),
                    "NodeResourceOwnershipConflict"
                );
                crate::install::delete_snapshot(&cm, &object).await?;
            }
        }
        for object in pods
            .list(
                &ListParams::default().labels(&format!("{LABEL}={id},kimage.dev/role=node-proxy")),
            )
            .await?
        {
            let wanted = resources
                .iter()
                .any(|v| v["kind"] == "Pod" && v["metadata"]["name"] == object.name_any());
            if !wanted {
                lease.check().await?;
                ensure!(
                    object.annotations().get("kimage.dev/manager") == Some(manager),
                    "NodeResourceOwnershipConflict"
                );
                crate::install::delete_foreground_snapshot(&pods, &object).await?;
                lifecycle::wait_deleted(&pods, &object.name_any()).await?;
            }
        }
        for arch in ["amd64", "arm64"] {
            let name = format!("{id}-builder-{arch}");
            let desired = resources
                .iter()
                .find(|v| v["kind"] == "Pod" && v["metadata"]["name"] == name);
            let current = pods.get_opt(&name).await?;
            let changed = match (&current, desired) {
                (Some(c), Some(d)) => {
                    crate::install::verify_declared(d, c).is_err()
                        || c.annotations()
                            .get("kimage.dev/node-uid")
                            .map(String::as_str)
                            != d["metadata"]["annotations"]["kimage.dev/node-uid"].as_str()
                }
                (None, None) => false,
                _ => true,
            };
            if !changed {
                continue;
            }
            // No new client can be approved while worker identity is changing.
            let catalog_name = format!("{id}-builders-{arch}");
            if let Some(old) = cm.get_opt(&catalog_name).await? {
                ensure!(
                    old.labels().get(LABEL) == Some(id),
                    "BuilderCatalogOwnershipConflict"
                );
                crate::install::delete_snapshot(&cm, &old).await?;
            }
            let mut native = settings.clone();
            native.architecture = Some(arch.into());
            let worker =
                crate::direct::WorkerLease::acquire(&config, &native, &identity[7..]).await?;
            let result = async {
                lease.check().await?;
                worker.check().await?;
                fence_clients(&config, arch).await?;
                if let Some(current) = current {
                    ensure!(
                        current.labels().get(LABEL) == Some(id)
                            && current.annotations().get("kimage.dev/manager") == Some(manager),
                        "BuilderPodOwnershipConflict"
                    );
                    crate::install::delete_foreground_snapshot(&pods, &current).await?;
                    lifecycle::wait_deleted(&pods, &name).await?;
                }
                Ok::<(), anyhow::Error>(())
            }
            .await;
            let released = worker.release().await;
            result?;
            released?;
        }
        // Supporting resources precede workers; publish endpoint/catalog approval
        // only after the actual native process is Ready.
        let mut pending = false;
        resources.sort_by_key(|v| {
            if v["kind"] == "Pod" {
                1
            } else if v["kind"] == "Job" {
                2
            } else if v["metadata"]["name"].as_str().is_some_and(|n| {
                n.ends_with("-endpoint") || n.starts_with(&format!("{id}-builders-"))
            }) {
                3
            } else {
                0
            }
        });
        for resource in &resources {
            let name = resource["metadata"]["name"].as_str().unwrap();
            if resource["kind"] == "ConfigMap" && name.ends_with("-endpoint") {
                let pod_name = format!("{}-proxy", name.strip_suffix("-endpoint").unwrap());
                if !pods.get_opt(&pod_name).await?.as_ref().is_some_and(ready) {
                    pending = true;
                    continue;
                }
            }
            if resource["kind"] == "ConfigMap" && name.starts_with(&format!("{id}-builders-")) {
                let arch = name.strip_prefix(&format!("{id}-builders-")).unwrap();
                let check = resources
                    .iter()
                    .find(|v| {
                        v["kind"] == "Job"
                            && v["metadata"]["name"].as_str().is_some_and(|n| {
                                n.starts_with(&format!("{id}-builder-check-{arch}-"))
                            })
                    })
                    .context("MissingNativeWorkerCheck")?;
                let check_name = check["metadata"]["name"].as_str().unwrap();
                let job = context.api_at("Job", Some(ns))?.get_opt(check_name).await?;
                if job
                    .as_ref()
                    .is_none_or(|j| j.data["status"]["succeeded"].as_u64().unwrap_or(0) < 1)
                {
                    pending = true;
                    continue;
                }
                if !pods
                    .get_opt(&format!("{id}-builder-{arch}"))
                    .await?
                    .as_ref()
                    .is_some_and(ready)
                {
                    pending = true;
                    continue;
                }
            }
            lease.check().await?;
            lifecycle::replace_owned(&context, resource, manager).await?;
        }
        for arch in ["amd64", "arm64"] {
            if workers.contains_key(arch) {
                continue;
            }
            for name in [
                format!("{id}-builders-{arch}"),
                format!("{id}-builder-template-{arch}"),
            ] {
                if let Some(old) = cm.get_opt(&name).await? {
                    ensure!(
                        old.labels().get(LABEL) == Some(id),
                        "BuilderCatalogOwnershipConflict"
                    );
                    lease.check().await?;
                    crate::install::delete_snapshot(&cm, &old).await?;
                }
            }
        }
        if options.worker_nodes != workers {
            options.worker_nodes = workers;
            let mut updated = state.clone();
            let mut record = state_record;
            record["options"] = serde_json::to_value(&options)?;
            updated
                .data
                .as_mut()
                .unwrap()
                .insert("installation.json".into(), serde_json::to_string(&record)?);
            lease.check().await?;
            maps.replace(&state.name_any(), &PostParams::default(), &updated)
                .await?;
        }
        ensure!(!pending, "NativeNodeReadinessPending");
        lifecycle::cleanup_worker_checks(&context, &resources, manager, &lease).await?;
        Ok(())
    }
    .await;
    let released = lease.release().await;
    let reason = result
        .as_ref()
        .err()
        .map(|e| {
            e.to_string()
                .split(':')
                .next()
                .unwrap_or("NodeReconcileFailed")
                .to_owned()
        })
        .unwrap_or_else(|| "Ready".into());
    let safe = if reason.len() <= 64 && reason.bytes().all(|b| b.is_ascii_alphanumeric()) {
        reason
    } else {
        "NodeReconcileFailed".into()
    };
    let name = format!("{id}-node-status");
    let state = maps.get(&format!("{id}-installation")).await?;
    let owner = json!({"apiVersion":"v1","kind":"ConfigMap","name":state.name_any(),"uid":state.uid(),"controller":false,"blockOwnerDeletion":false});
    let mut diagnostic: ConfigMap = serde_json::from_value(
        json!({"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":name,"labels":{LABEL:id},"annotations":{"kimage.dev/manager":state.annotations().get("kimage.dev/manager")},"ownerReferences":[owner]},"data":{"status.json":json!({"schema":1,"reason":safe,"checkedAt":time::OffsetDateTime::now_utc().unix_timestamp()}).to_string()}}),
    )?;
    if let Some(existing) = maps.get_opt(&name).await? {
        lifecycle::owned(&existing, id)?;
        diagnostic.metadata.resource_version = existing.resource_version();
        maps.replace(&name, &PostParams::default(), &diagnostic)
            .await?;
    } else {
        maps.create(&PostParams::default(), &diagnostic).await?;
    }
    result.and(released)
}
async fn fence_clients(config: &crate::reconcile::Config, arch: &str) -> Result<()> {
    let settings = config
        .direct
        .as_ref()
        .context("MissingInstallationIdentity")?;
    let pods: Api<Pod> = if config.namespace == "*" {
        Api::all(config.client.clone())
    } else {
        Api::namespaced(config.client.clone(), &config.namespace)
    };
    let page = pods
        .list(
            &ListParams::default()
                .labels(&format!(
                    "kimage.dev/build-installation={},kimage.dev/role=builder-client",
                    settings.installation
                ))
                .limit(1024),
        )
        .await?;
    ensure!(
        page.metadata
            .continue_
            .as_ref()
            .is_none_or(String::is_empty),
        "TooManyActiveBuilds"
    );
    for pod in page {
        let ns = pod.namespace().context("MissingBuildNamespace")?;
        ensure!(
            config.scope.as_ref().is_none_or(|s| s.allows(&ns)),
            "BuildScopeConflict"
        );
        let id = pod
            .annotations()
            .get("kimage.dev/build-snapshot-id")
            .context("MissingBuildAttempt")?;
        crate::direct::client_name(id)?;
        let maps: Api<ConfigMap> = Api::namespaced(config.client.clone(), &ns);
        let record = maps.get(&format!("build-{}-session", &id[..20])).await?;
        ensure!(
            record.owner_references() == pod.owner_references(),
            "BuildSessionOwnershipConflict"
        );
        let record: crate::direct_client::Record = serde_json::from_str(
            record
                .data
                .as_ref()
                .and_then(|d| d.get("record.json"))
                .context("MissingBuildSessionRecord")?,
        )?;
        if record.session.platform.architecture == arch {
            let mut scoped = config.clone();
            scoped.namespace = ns;
            crate::direct::remove_client(&scoped, id).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn runtime_ownership_is_helm_adoptable_without_accepting_foreign_template_owners_or_spec_edits()
    {
        let state:ConfigMap=serde_json::from_value(json!({"metadata":{"name":"dev-installation","namespace":"system","uid":"installation-uid","labels":{LABEL:"dev"},"annotations":{"kimage.dev/manager":"helm","meta.helm.sh/release-name":"different-release-name","meta.helm.sh/release-namespace":"system"}}})).unwrap();
        let pod = json!({"apiVersion":"v1","kind":"Pod","metadata":{"name":"dev-builder-amd64","namespace":"system","labels":{LABEL:"dev"},"annotations":{"kimage.dev/manager":"helm","kimage.dev/node-uid":"node-uid"}},"spec":{"containers":[{"name":"builder","image":"pinned"}]}});
        let original = json!({"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":"dev-builder-template-amd64","namespace":"system","labels":{LABEL:"dev"},"annotations":{"kimage.dev/manager":"helm"}},"data":{"pod.json":serde_json::to_string(&pod).unwrap()}});
        let mut resources = vec![original.clone()];
        decorate(&mut resources, &state).unwrap();
        let expected = &resources[0];
        assert_eq!(
            expected["metadata"]["annotations"]["meta.helm.sh/release-name"],
            "different-release-name"
        );
        assert!(equivalent_template(expected, &original));
        assert!(equivalent_template(expected, expected));
        let mut forged = expected.clone();
        let mut body: Value =
            serde_json::from_str(forged["data"]["pod.json"].as_str().unwrap()).unwrap();
        body["metadata"]["ownerReferences"][0]["uid"] = json!("foreign-uid");
        forged["data"]["pod.json"] = json!(serde_json::to_string(&body).unwrap());
        assert!(!equivalent_template(expected, &forged));
        let mut edited = pod;
        edited["spec"]["containers"][0]["image"] = json!("unapproved");
        forged["data"]["pod.json"] = json!(serde_json::to_string(&edited).unwrap());
        assert!(!equivalent_template(expected, &forged));
    }
}

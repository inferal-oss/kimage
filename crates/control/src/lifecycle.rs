//! Explicit, resumable whole-installation transitions. Retained data is never migrated.
use crate::{
    component_images::ImageCatalog,
    install::{Installer, LABEL},
    operation_lease::OperationLease,
    pull_install::{Options, Plan, apply, credentials::Credentials},
};
use anyhow::{Context, Result, ensure};
use k8s_openapi::api::core::v1::{ConfigMap, Node, Pod, Secret};
use kube::{
    Api, Client, ResourceExt,
    api::{DynamicObject, ListParams, Patch, PatchParams, PostParams},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path, time::Duration};

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub enum Phase {
    Prepared,
    Quiescing,
    Applying,
    Verifying,
    Complete,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Journal {
    pub schema: u32,
    pub identity: String,
    pub installation_uid: String,
    pub manager: String,
    pub source: ImageCatalog,
    pub target: ImageCatalog,
    pub options: Options,
    pub phase: Phase,
    pub retained: BTreeMap<String, String>,
    pub configuration: String,
    #[serde(default)]
    pub roots: Vec<RootCheck>,
    #[serde(default)]
    pub node_policy: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RootCheck {
    pub namespace: String,
    pub name: String,
    pub uid: String,
    pub image_uid: String,
    pub root: kimage_core::oci::Descriptor,
}
/// The only initially supported edge. Both releases read/write the same API,
/// registry filesystem, native build journal, and serving protocol.
pub fn release_metadata() -> Result<Value> {
    let metadata: Value = serde_json::from_str(include_str!("release-compatibility.json"))?;
    ensure!(
        metadata["schema"] == 1
            && metadata["release"] == env!("CARGO_PKG_VERSION")
            && metadata["formats"]["apiSchemaRevision"] == crate::schema::REVISION,
        "InvalidCompiledReleaseContract"
    );
    Ok(metadata)
}
pub fn compatible(source: &ImageCatalog, target: &ImageCatalog) -> Result<()> {
    source.validate(&source.release, false)?;
    target.validate(&target.release, false)?;
    let metadata = release_metadata()?;
    let transitions = metadata["transitions"]
        .as_array()
        .context("MissingReleaseCompatibilityEdges")?;
    ensure!(
        transitions.iter().any(|edge| edge["from"] == source.release
            && edge["to"] == target.release
            || edge["rollbackSafe"] == true
                && edge["from"] == target.release
                && edge["to"] == source.release),
        "UnsupportedReleaseTransition: no tested compatibility edge for these exact releases"
    );
    ensure!(
        source.api_schema_revision == 5 && target.api_schema_revision == 5,
        "ReleaseSchemaMigrationRequired"
    );
    for (role, version) in [
        (
            "registry",
            metadata["components"]["registry"]
                .as_str()
                .context("InvalidReleaseContract")?,
        ),
        (
            "gateway",
            metadata["formats"]["servingProtocol"]
                .as_str()
                .context("InvalidReleaseContract")?,
        ),
        (
            "proxy",
            metadata["formats"]["servingProtocol"]
                .as_str()
                .context("InvalidReleaseContract")?,
        ),
        (
            "builder",
            metadata["components"]["builder"]
                .as_str()
                .context("InvalidReleaseContract")?,
        ),
    ] {
        ensure!(
            source.components[role].version == version
                && target.components[role].version == version,
            "IncompatibleDataOrServingProtocol"
        );
    }
    Ok(())
}
pub(crate) fn owned(map: &ConfigMap, id: &str) -> Result<()> {
    ensure!(
        map.labels().get(LABEL).is_some_and(|v| v == id),
        "InstallationOwnershipConflict"
    );
    Ok(())
}
pub async fn state(client: Client, ns: &str, id: &str) -> Result<(ConfigMap, Options, String)> {
    let maps: Api<ConfigMap> = Api::namespaced(client, ns);
    let map = maps.get(&format!("{id}-installation")).await?;
    owned(&map, id)?;
    let value: Value = serde_json::from_str(
        map.data
            .as_ref()
            .and_then(|d| d.get("installation.json"))
            .context("MissingInstallationState")?,
    )?;
    ensure!(
        value["schema"] == 1 && value["delivery"] == "ImagePull",
        "UnsupportedInstallationState"
    );
    let options: Options = serde_json::from_value(value["options"].clone())?;
    options.validate()?;
    ensure!(
        options.installation == id && options.namespace == ns,
        "InstallationIdentityConflict"
    );
    let release = value["release"]
        .as_str()
        .context("MissingInstalledRelease")?
        .to_owned();
    Ok((map, options, release))
}
pub async fn journal(client: Client, ns: &str, id: &str) -> Result<Option<Journal>> {
    let maps: Api<ConfigMap> = Api::namespaced(client, ns);
    let Some(map) = maps.get_opt(&format!("{id}-operation")).await? else {
        return Ok(None);
    };
    owned(&map, id)?;
    let raw = map
        .data
        .as_ref()
        .and_then(|d| d.get("operation.json"))
        .context("MissingInstallationOperation")?;
    ensure!(raw.len() <= 512 * 1024, "InstallationOperationTooLarge");
    let value: Value = serde_json::from_str(raw)?;
    let record: Journal = match serde_json::from_value(value.clone()) {
        Ok(record) => record,
        Err(_) if value["phase"] == "Complete" => return Ok(None),
        Err(_) => anyhow::bail!(
            "UnsupportedActiveOperation: resume with the CLI that created this journal"
        ),
    };
    if record.schema != 1 && record.phase == Phase::Complete {
        return Ok(None);
    }
    ensure!(
        record.schema == 1 && record.options.namespace == ns && record.options.installation == id,
        "InstallationOperationIdentityConflict"
    );
    ensure!(
        record.identity == identity(&record.source, &record.target, &record.installation_uid)?,
        "InstallationOperationIdentityConflict"
    );
    Ok(Some(record))
}
pub async fn active(client: Client, ns: &str, id: &str) -> Result<bool> {
    let map = Api::<ConfigMap>::namespaced(client, ns)
        .get_opt(&format!("{id}-operation"))
        .await?;
    let Some(map) = map else { return Ok(false) };
    owned(&map, id)?;
    let raw = map
        .data
        .as_ref()
        .and_then(|d| d.get("operation.json"))
        .context("MissingInstallationOperation")?;
    ensure!(raw.len() <= 512 * 1024, "InstallationOperationTooLarge");
    let record: Value = serde_json::from_str(raw)?;
    // Runtime gating is forward-compatible. A newer CLI may safely roll back to
    // this operator while retaining a completed journal it cannot fully parse.
    // Unknown active formats remain fenced and require the matching newer CLI.
    Ok(record["phase"] != "Complete")
}
fn identity(source: &ImageCatalog, target: &ImageCatalog, uid: &str) -> Result<String> {
    Ok(kimage_core::oci::digest(&serde_json::to_vec(&(source, target, uid))?)[7..].into())
}
pub(crate) fn is_rollback(source: &str, target: &str) -> Result<bool> {
    Ok(kimage_core::selector::exact(target)? < kimage_core::selector::exact(source)?)
}
async fn save(client: Client, j: &Journal) -> Result<()> {
    let maps: Api<ConfigMap> = Api::namespaced(client, &j.options.namespace);
    let name = format!("{}-operation", j.options.installation);
    let mut desired: ConfigMap = serde_json::from_value(
        json!({"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":name,"labels":{LABEL:j.options.installation},"ownerReferences":[{"apiVersion":"v1","kind":"ConfigMap","name":format!("{}-installation",j.options.installation),"uid":j.installation_uid,"controller":false,"blockOwnerDeletion":false}]},"data":{"operation.json":serde_json::to_string(j)?}}),
    )?;
    if let Some(existing) = maps.get_opt(&name).await? {
        owned(&existing, &j.options.installation)?;
        desired.metadata.uid = existing.uid();
        desired.metadata.resource_version = existing.resource_version();
        maps.replace(&name, &PostParams::default(), &desired)
            .await?;
    } else {
        maps.create(&PostParams::default(), &desired).await?;
    }
    Ok(())
}
pub(crate) async fn nodes(client: Client) -> Result<Vec<Node>> {
    let page = Api::<Node>::all(client)
        .list(&ListParams::default().limit(1024))
        .await?;
    ensure!(
        page.metadata
            .continue_
            .as_ref()
            .is_none_or(String::is_empty),
        "TooManyClusterNodes"
    );
    Ok(page.items)
}
async fn retained(context: &Installer, options: &Options) -> Result<BTreeMap<String, String>> {
    let mut result = BTreeMap::new();
    let claim = options
        .existing_claim
        .clone()
        .unwrap_or_else(|| format!("{}-data", options.installation));
    let pvc = context
        .api_at("PersistentVolumeClaim", Some(&options.namespace))?
        .get(&claim)
        .await?;
    ensure!(
        pvc.data["status"]["phase"] == "Bound",
        "RegistryStorageNotBound"
    );
    result.insert(
        format!("PersistentVolumeClaim/{claim}"),
        pvc.uid().context("MissingRetainedUid")?,
    );
    let volume = pvc.data["spec"]["volumeName"]
        .as_str()
        .context("MissingRegistryVolume")?;
    let pv = context
        .api_at("PersistentVolume", None)?
        .get(volume)
        .await?;
    result.insert(
        format!("PersistentVolume/{volume}"),
        pv.uid().context("MissingRetainedUid")?,
    );
    let secrets: Api<Secret> = Api::namespaced(context.client.clone(), &options.namespace);
    for role in ["registry", "admission", "builder"] {
        let name = format!("{}-{role}-credentials", options.installation);
        let secret = secrets.get(&name).await?;
        ensure!(
            secret.labels().get(LABEL) == Some(&options.installation),
            "CredentialOwnershipConflict"
        );
        result.insert(
            format!("Secret/{name}"),
            format!(
                "{}:{}",
                secret.uid().context("MissingCredentialUid")?,
                kimage_core::oci::digest(&serde_json::to_vec(&secret.data)?)
            ),
        );
    }
    for crd in crate::schema::definitions()? {
        let api=Api::<k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition>::all(context.client.clone());
        let current = api.get(&crd.name_any()).await?;
        crate::schema::compatibility(&current, &crd)?;
        result.insert(
            format!("CustomResourceDefinition/{}", crd.name_any()),
            current.uid().context("MissingRetainedUid")?,
        );
    }
    Ok(result)
}
fn manager(state: &ConfigMap) -> Result<String> {
    let manager = state
        .annotations()
        .get("kimage.dev/manager")
        .context("MissingInstallationManager")?
        .clone();
    ensure!(
        matches!(manager.as_str(), "cli" | "helm"),
        "UnsupportedInstallationManager"
    );
    Ok(manager)
}
fn registry_identity(state: &ConfigMap, nodes: &[Node], options: &Options) -> Result<()> {
    let value: Value = serde_json::from_str(
        &state.data.as_ref().context("MissingInstallationState")?["installation.json"],
    )?;
    let primary = nodes
        .iter()
        .find(|n| Some(n.name_any()) == options.registry_node)
        .context("RetainedRegistryNodeUnavailable")?;
    ensure!(
        value["registryNodeUid"].as_str() == primary.uid().as_deref(),
        "RegistryNodeIdentityChanged: recover retained storage explicitly"
    );
    Ok(())
}
pub async fn transition(
    client: Client,
    ns: &str,
    id: &str,
    target: &ImageCatalog,
    check: bool,
    rollback: bool,
    chart: Option<&Path>,
) -> Result<Value> {
    let (installed, options, release) = state(client.clone(), ns, id).await?;
    let current_journal = journal(client.clone(), ns, id).await?;
    let resumed = current_journal
        .as_ref()
        .is_some_and(|j| j.phase != Phase::Complete);
    let maps: Api<ConfigMap> = Api::namespaced(client.clone(), ns);
    let policy = maps.get(&format!("{id}-component-images")).await?;
    owned(&policy, id)?;
    let source = if resumed {
        current_journal.as_ref().unwrap().source.clone()
    } else {
        ImageCatalog::parse(
            policy
                .data
                .as_ref()
                .and_then(|d| d.get("images.json"))
                .context("MissingInstalledImageCatalog")?
                .as_bytes(),
            &release,
            false,
        )?
    };
    if !resumed && source.release == target.release {
        target.validate(&target.release, false)?;
        ensure!(
            serde_json::to_value(&source)? == serde_json::to_value(target)?,
            "InstalledTargetCatalogConflict"
        );
        return Ok(
            json!({"target":target.release,"alreadyInstalled":true,"phase":"Complete","checks":"passed"}),
        );
    }
    compatible(&source, target)?;
    ensure!(
        is_rollback(&source.release, &target.release)? == rollback,
        "TransitionDirectionConflict"
    );
    let all_nodes = nodes(client.clone()).await?;
    registry_identity(&installed, &all_nodes, &options)?;
    let context = apply::context(
        client.clone(),
        &options,
        options
            .registry_node
            .as_deref()
            .context("MissingRegistryPlacement")?,
    )?;
    let configuration_map = maps.get(&format!("{id}-config")).await?;
    crate::configuration::parse(&configuration_map, id)?;
    let configuration = configuration_map.data.as_ref().unwrap()["configuration.json"].clone();
    let helm_values = if manager(&installed)? == "helm" {
        ensure!(
            std::env::var_os("KIMAGE_API_SERVER").is_none()
                && std::env::var_os("KIMAGE_TLS_SERVER_NAME").is_none(),
            "HelmRequiresMatchingKubeconfig: put endpoint overrides in a verified kubeconfig shared by both clients"
        );
        let release_name = installed
            .annotations()
            .get("meta.helm.sh/release-name")
            .context("MissingHelmReleaseIdentity")?;
        Some(serde_json::from_slice::<Value>(
            &helm_output(&[
                "get".into(),
                "values".into(),
                release_name.clone(),
                "--namespace".into(),
                ns.into(),
                "--all".into(),
                "--output".into(),
                "json".into(),
            ])
            .await?,
        )?)
    } else {
        None
    };
    let initial_policy = maps
        .get_opt(&format!("{id}-node-policy"))
        .await?
        .and_then(|m| m.data)
        .and_then(|d| d.get("policy.json").cloned());
    let initial_policy = if initial_policy.is_none() {
        if let Some(values) = &helm_values {
            let pins = serde_json::from_value::<BTreeMap<String, String>>(
                values.get("workerNodes").cloned().unwrap_or(json!({})),
            )?;
            if pins.is_empty() {
                None
            } else {
                Some(serde_json::to_string(&crate::node_reconcile::Policy {
                    worker_pins: pins,
                    ..Default::default()
                })?)
            }
        } else {
            None
        }
    } else {
        initial_policy
    };
    let mut record = if resumed {
        let j = current_journal.unwrap();
        ensure!(
            serde_json::to_value(&j.target)? == serde_json::to_value(target)?
                && j.installation_uid == installed.uid().context("MissingInstallationUid")?
                && j.manager == manager(&installed)?,
            "AnotherInstallationOperationPending: resume its exact target first"
        );
        j
    } else {
        Journal {
            schema: 1,
            identity: identity(
                &source,
                target,
                &installed.uid().context("MissingInstallationUid")?,
            )?,
            installation_uid: installed.uid().unwrap(),
            manager: manager(&installed)?,
            source,
            target: target.clone(),
            options: options.clone(),
            phase: Phase::Prepared,
            retained: retained(&context, &options).await?,
            configuration,
            roots: sample_roots(client.clone(), &options).await?,
            node_policy: initial_policy,
        }
    };
    ensure!(
        retained(&context, &options).await? == record.retained,
        "RetainedInstallationIdentityChanged"
    );
    ensure!(
        configuration_map.data.as_ref().unwrap()["configuration.json"] == record.configuration,
        "AdministratorPolicyChangedDuringOperation"
    );
    check_root_identities(client.clone(), &record.roots).await?;
    let credentials = Credentials::resolve(client.clone(), ns, id).await?;
    let current_policy = maps.get_opt(&format!("{id}-node-policy")).await?;
    let expected_policy = record
        .node_policy
        .as_ref()
        .map(|raw| serde_json::from_str::<crate::node_reconcile::Policy>(raw))
        .transpose()?
        .unwrap_or_default();
    let node_policy = if current_policy.is_none()
        && record.manager == "helm"
        && (record.target.release == "0.4.1" && record.phase == Phase::Applying
            || record.source.release == "0.4.1"
                && crate::component_images::native_lifecycle(&record.target.release)?)
    {
        expected_policy.clone()
    } else {
        crate::node_reconcile::policy(client.clone(), ns, id).await?
    };
    ensure!(
        node_policy == expected_policy,
        "NodePolicyChangedDuringOperation"
    );
    ensure!(
        !rollback || target.release != "0.4.1" || node_policy.node_selector.is_empty(),
        "TargetCannotHonorNodeSelectionPolicy: restore the default node selection before rolling back to 0.4.1"
    );
    let (selected, workers) = crate::node_reconcile::select(&options, &node_policy, &all_nodes)?;
    let mut planned_options = options.clone();
    planned_options.worker_nodes = workers;
    let mut plan = Plan::new(&planned_options, target, &selected, &credentials)?;
    if let Some(policy) = maps.get_opt(&format!("{id}-node-policy")).await? {
        owned(&policy, id)?;
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
    for resource in &mut plan.resources {
        if resource["kind"] == "ConfigMap"
            && resource["metadata"]["name"] == format!("{id}-node-policy")
            && current_policy.is_none()
            && let Some(raw) = &record.node_policy
        {
            resource["data"]["policy.json"] = json!(raw);
        }
        if resource["kind"] == "ConfigMap" && resource["metadata"]["name"] == format!("{id}-config")
        {
            resource["data"]["configuration.json"] = json!(record.configuration);
        }
    }
    if crate::component_images::native_lifecycle(&target.release)? {
        crate::node_reconcile::decorate(&mut plan.resources, &installed)?;
    }
    let mut chart_file = None;
    let chart_path = if record.manager == "helm" {
        ensure!(
            installed
                .annotations()
                .get("meta.helm.sh/release-namespace")
                .is_some_and(|v| v == ns),
            "HelmReleaseNamespaceConflict"
        );
        helm_update_mode().await?;
        let values = helm_values.as_ref().context("MissingHelmReleaseValues")?;
        let pinned = |catalog: &ImageCatalog| {
            values["images"]["repository"] == catalog.repository
                && catalog
                    .components
                    .iter()
                    .all(|(role, c)| values["images"]["digests"][role] == c.digest)
        };
        ensure!(
            pinned(&record.source) || resumed && pinned(&record.target),
            "HelmHistoryImageCatalogConflict"
        );
        // Helm's supported pending-revision recovery replays a stable manifest.
        // Refuse before quiescing if it would restore stale credential or policy
        // bytes. Reapplying the current chart first records the live values.
        let release_name = installed
            .annotations()
            .get("meta.helm.sh/release-name")
            .context("MissingHelmReleaseIdentity")?;
        let history = helm_history(release_name, ns).await?;
        let revision = recovery_revision(&history)?;
        verify_helm_recovery(&context, &record, release_name, revision).await?;
        ensure!(
            std::env::var_os("KIMAGE_API_SERVER").is_none()
                && std::env::var_os("KIMAGE_TLS_SERVER_NAME").is_none(),
            "HelmRequiresMatchingKubeconfig: put API endpoint overrides in a verified kubeconfig shared by both clients"
        );
        {
            let path = if let Some(path) = chart {
                path.to_owned()
            } else {
                let bytes = crate::release_bundle::small_release_asset(
                    &target.release,
                    &format!("kimage-{}.tgz", target.release),
                )
                .await?;
                let file = tempfile::NamedTempFile::new()?;
                std::fs::write(file.path(), bytes)?;
                let path = file.path().to_owned();
                chart_file = Some(file);
                path
            };
            let metadata = helm_output(&[
                "show".into(),
                "chart".into(),
                path.to_str().context("InvalidHelmChartPath")?.into(),
            ])
            .await?;
            let metadata = std::str::from_utf8(&metadata)?;
            for key in ["version", "appVersion"] {
                ensure!(
                    metadata.lines().any(|l| l
                        .strip_prefix(&format!("{key}: "))
                        .is_some_and(|v| v.trim_matches('"') == target.release)),
                    "HelmTargetChartVersionConflict"
                );
            }
            Some(path)
        }
    } else {
        None
    };
    let summary = json!({"source":record.source.release,"target":record.target.release,"manager":record.manager,"resuming":resumed,"phase":record.phase,"compatibility":{"apiSchema":5,"installationState":1,"buildJournal":1,"serving":"0.3.0","registry":"3.1.2","storageMigration":false},"checks":"passed"});
    if check {
        return Ok(summary);
    }
    let lease = OperationLease::acquire_wait(
        client.clone(),
        ns,
        id,
        &format!("{id}-lifecycle"),
        &record.identity,
    )
    .await?;
    let result=async {
        // Re-read after acquiring the lock. A stale check must never replace an
        // operation completed by another client between planning and locking.
        let now=journal(client.clone(),ns,id).await?;
        ensure!(now.as_ref().is_none_or(|j|j.phase==Phase::Complete || j.identity==record.identity),"AnotherInstallationOperationPending");
        let (latest,latest_options,latest_release)=state(client.clone(),ns,id).await?;
        ensure!(serde_json::to_value(&latest_options)?==serde_json::to_value(&record.options)?,"InstallationOptionsChangedDuringPreflight");
        ensure!(latest.uid().as_deref()==Some(&record.installation_uid) && (latest_release==record.source.release || resumed && latest_release==record.target.release),"InstallationChangedDuringPreflight");
        if now.as_ref().is_some_and(|j|j.phase==Phase::Complete && j.identity==record.identity) && latest_release==record.target.release {
            return Ok(json!({"target":record.target.release,"phase":"Complete","alreadyInstalled":true}));
        }
        save(client.clone(),&record).await?;
        if matches!(record.phase,Phase::Prepared|Phase::Quiescing) {
            record.phase=Phase::Quiescing;save(client.clone(),&record).await?;
            quiesce(&context,&options,&lease).await?;
            record.phase=Phase::Applying;save(client.clone(),&record).await?;
        }
        if record.phase==Phase::Applying {
            if record.manager=="helm" { helm_transition(&context,&record,chart_path.as_deref(),&lease).await?; }
            else {
                for resource in &plan.resources {lease.check().await?;replace_owned(&context,resource,"cli").await?;}
                for resource in &plan.webhooks {lease.check().await?;replace_owned(&context,resource,"cli").await?;}
            }
            record.phase=Phase::Verifying;save(client.clone(),&record).await?;
        }
        lease.check().await?;
        apply::wait_ready(&context,&plan).await?;
        cleanup_worker_checks(&context,&plan.resources,&record.manager,&lease).await?;
        ensure!(retained(&context,&options).await?==record.retained,"RetainedInstallationIdentityChanged");
        let (latest,_,release)=state(client.clone(),ns,id).await?;
        ensure!(latest.uid().as_deref()==Some(&record.installation_uid) && release==record.target.release,"InstallationVerificationFailed");
        verify_retained_roots(client.clone(),&record).await?;
        record.phase=Phase::Complete;save(client.clone(),&record).await?;
        Ok(json!({"source":record.source.release,"target":record.target.release,"manager":record.manager,"phase":record.phase,"retainedIdentitiesVerified":true}))
    }.await;
    let released = lease.release().await;
    drop(chart_file);
    result.and_then(|v| released.map(|_| v))
}
pub(crate) async fn cleanup_worker_checks(
    context: &Installer,
    resources: &[Value],
    manager: &str,
    lease: &OperationLease,
) -> Result<()> {
    let api = context.api_at("Job", Some(&context.options.namespace))?;
    let id = &context.options.installation;
    let jobs = api
        .list(
            &ListParams::default()
                .labels(&format!("{LABEL}={id}"))
                .limit(512),
        )
        .await?;
    ensure!(
        jobs.metadata
            .continue_
            .as_ref()
            .is_none_or(String::is_empty),
        "TooManyWorkerChecks"
    );
    for job in jobs {
        let name = job.name_any();
        if !name.starts_with(&format!("{id}-builder-check-"))
            || resources
                .iter()
                .any(|v| v["kind"] == "Job" && v["metadata"]["name"] == name)
        {
            continue;
        }
        ensure!(
            job.annotations()
                .get("kimage.dev/manager")
                .is_some_and(|v| v == manager),
            "WorkerCheckOwnershipConflict"
        );
        lease.check().await?;
        crate::install::delete_foreground_snapshot(&api, &job).await?;
        wait_deleted(&api, &name).await?;
    }
    Ok(())
}
async fn sample_roots(client: Client, options: &Options) -> Result<Vec<RootCheck>> {
    let api: Api<crate::api::ImageRelease> = if let Some(ns) = &options.application_namespace {
        Api::namespaced(client.clone(), ns)
    } else {
        Api::all(client.clone())
    };
    let mut token: Option<String> = None;
    let mut sampled = Vec::new();
    let mut pages = 0;
    loop {
        let mut params = ListParams::default().limit(256);
        if let Some(next) = &token {
            params = params.continue_token(next);
        }
        let page = api.list_metadata(&params).await?;
        pages += 1;
        ensure!(pages <= 256, "RetainedCatalogScanLimit");
        for item in page.items {
            let ns = item.namespace().context("MissingReleaseNamespace")?;
            if !options.scope().allows(&ns) {
                continue;
            }
            sampled.push((
                ns,
                item.name_any(),
                item.uid().context("MissingReleaseUid")?,
            ));
            sampled.sort();
            if sampled.len() > 6 {
                sampled.remove(3);
            }
        }
        token = page.metadata.continue_.filter(|v| !v.is_empty());
        if token.is_none() {
            break;
        }
    }
    let mut roots = Vec::new();
    for (ns, name, uid) in sampled {
        let release = Api::<crate::api::ImageRelease>::namespaced(client.clone(), &ns)
            .get(&name)
            .await?;
        ensure!(
            release.uid().as_deref() == Some(&uid),
            "RetainedReleaseIdentityChanged"
        );
        if release.status.as_ref().is_some_and(|s| s.verified) {
            roots.push(RootCheck {
                namespace: ns,
                name,
                uid,
                image_uid: release.spec.image_uid,
                root: release.spec.root,
            });
        }
    }
    Ok(roots)
}
async fn check_root_identities(client: Client, roots: &[RootCheck]) -> Result<()> {
    ensure!(roots.len() <= 6, "InvalidRetainedRootChecks");
    for root in roots {
        let current = Api::<crate::api::ImageRelease>::namespaced(client.clone(), &root.namespace)
            .get(&root.name)
            .await?;
        ensure!(
            current.uid().as_deref() == Some(&root.uid)
                && current.spec.image_uid == root.image_uid
                && current.spec.root == root.root
                && current.status.as_ref().is_some_and(|s| s.verified),
            "RetainedReleaseIdentityChanged"
        );
    }
    Ok(())
}
async fn verify_retained_roots(client: Client, journal: &Journal) -> Result<()> {
    check_root_identities(client.clone(), &journal.roots).await?;
    if journal.roots.is_empty() {
        return Ok(());
    }
    let options = &journal.options;
    let pods = Api::<Pod>::namespaced(client.clone(), &options.namespace)
        .list(&ListParams::default().labels(&format!(
            "{LABEL}={},kimage.dev/role=registry",
            options.installation
        )))
        .await?;
    ensure!(pods.items.len() == 1, "RegistryEndpointPending");
    let tunnel = crate::tunnel::Tunnel::open(
        client.clone(),
        &options.namespace,
        &pods.items[0].name_any(),
        5443,
    )
    .await?;
    let secret = Api::<Secret>::namespaced(client, &options.namespace)
        .get(&format!("{}-registry-credentials", options.installation))
        .await?;
    let data = secret.data.context("MissingRegistryCredentials")?;
    let ca = &data.get("ca.crt").context("MissingRegistryAuthority")?.0;
    let password = std::str::from_utf8(
        &data
            .get("reader-password")
            .context("MissingRegistryReader")?
            .0,
    )?
    .to_owned();
    for root in &journal.roots {
        let registry = crate::registry::Registry::new(
            format!(
                "https://{}-registry.{}.svc:5443/",
                options.installation, options.namespace
            )
            .parse()?,
            format!("{}/{}", root.namespace, root.image_uid),
            ca,
            Some(tunnel.address),
            "reader".into(),
            password.clone(),
        )?;
        registry.verify_retained_root(&root.root).await?;
    }
    Ok(())
}
/// Lease-guarded replacement of declared fields, preserving API-assigned identity.
pub(crate) async fn replace_owned(
    context: &Installer,
    desired: &Value,
    manager: &str,
) -> Result<()> {
    let kind = desired["kind"].as_str().context("MissingResourceKind")?;
    let name = desired["metadata"]["name"]
        .as_str()
        .context("MissingResourceName")?;
    let ns = apply::namespaced(kind).then(|| desired["metadata"]["namespace"].as_str().unwrap());
    let api = context.api_at(kind, ns)?;
    let Some(current) = api.get_opt(name).await? else {
        let object: DynamicObject = serde_json::from_value(desired.clone())?;
        api.create(&PostParams::default(), &object).await?;
        return Ok(());
    };
    ensure!(
        current.labels().get(LABEL) == Some(&context.options.installation)
            && current
                .annotations()
                .get("kimage.dev/manager")
                .is_some_and(|v| v == manager),
        "InstallationResourceOwnershipConflict: {kind}/{name}"
    );
    if crate::install::verify_declared(desired, &current).is_ok() {
        return Ok(());
    }
    // Existing credentials and retained volumes must only be checked, never replaced.
    ensure!(
        !matches!(kind, "Secret" | "PersistentVolumeClaim"),
        "RetainedResourceDrift: {kind}/{name}"
    );
    if matches!(kind, "Pod" | "Job") || current.data["immutable"] == true {
        crate::install::delete_foreground_snapshot(&api, &current).await?;
        wait_deleted(&api, name).await?;
        api.create(
            &PostParams::default(),
            &serde_json::from_value(desired.clone())?,
        )
        .await?;
    } else {
        let mut patch = desired.clone();
        patch["metadata"]["uid"] = json!(current.uid());
        patch["metadata"]["resourceVersion"] = json!(current.resource_version());
        api.patch(name, &PatchParams::default(), &Patch::Merge(patch))
            .await?;
    }
    Ok(())
}
pub(crate) async fn wait_deleted(api: &Api<DynamicObject>, name: &str) -> Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
    while api.get_opt(name).await?.is_some() {
        ensure!(
            tokio::time::Instant::now() < deadline,
            "ComponentFencingPending: {name}"
        );
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    Ok(())
}
async fn quiesce(context: &Installer, options: &Options, lease: &OperationLease) -> Result<()> {
    let deployments = context.api_at("Deployment", Some(&options.namespace))?;
    let name = format!("{}-operator", options.installation);
    let operator = deployments.get(&name).await?;
    ensure!(
        operator.labels().get(LABEL) == Some(&options.installation),
        "InstallationOwnershipConflict"
    );
    lease.check().await?;
    deployments.patch(&name,&PatchParams::default(),&Patch::Merge(json!({"metadata":{"uid":operator.uid(),"resourceVersion":operator.resource_version()},"spec":{"replicas":0}}))).await?;
    let pods: Api<Pod> = Api::namespaced(context.client.clone(), &options.namespace);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
    loop {
        lease.check().await?;
        let page = pods
            .list(&ListParams::default().labels(&format!(
                "{LABEL}={},kimage.dev/role=operator",
                options.installation
            )))
            .await?;
        if page.items.is_empty() {
            break;
        }
        ensure!(
            tokio::time::Instant::now() < deadline,
            "OperatorQuiescePending"
        );
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let all: Api<Pod> = match &options.application_namespace {
        Some(ns) => Api::namespaced(context.client.clone(), ns),
        None => Api::all(context.client.clone()),
    };
    let page = all
        .list(
            &ListParams::default()
                .labels(&format!(
                    "kimage.dev/build-installation={},kimage.dev/role=builder-client",
                    options.installation
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
        ensure!(options.scope().allows(&ns), "BuildScopeConflict");
        let id = pod
            .annotations()
            .get("kimage.dev/build-snapshot-id")
            .context("MissingBuildAttempt")?;
        let _ = crate::build_logs::archive(context.client.clone(), &ns, &pod, id).await;
        lease.check().await?;
        let api = context.api_at("Pod", Some(&ns))?;
        let dynamic: DynamicObject = serde_json::from_value(serde_json::to_value(&pod)?)?;
        ensure!(
            pod.owner_references()
                .iter()
                .any(|o| o.kind == "Image"
                    && Some(&o.uid) == pod.labels().get("kimage.dev/image-uid")),
            "BuildOwnershipConflict"
        );
        crate::install::delete_foreground_snapshot(&api, &dynamic).await?;
        wait_deleted(&api, &pod.name_any()).await?;
    }
    // Terminate every old worker before changing catalogs. A dead or partitioned
    // kubelet leaves the operation pending rather than allowing an unsafe steal.
    let api = context.api_at("Pod", Some(&options.namespace))?;
    let page = api
        .list(&ListParams::default().labels(&format!(
            "{LABEL}={},kimage.dev/role=builder",
            options.installation
        )))
        .await?;
    for pod in page {
        lease.check().await?;
        crate::install::delete_foreground_snapshot(&api, &pod).await?;
        wait_deleted(&api, &pod.name_any()).await?;
    }
    Ok(())
}
async fn helm_output(args: &[String]) -> Result<Vec<u8>> {
    let output = tokio::process::Command::new("helm")
        .args(args)
        .kill_on_drop(true)
        .output()
        .await
        .context("HelmExecutableRequired")?;
    if !output.status.success() {
        // Helm can include rendered credential objects in stderr. Return a
        // stable reason, never arbitrary chart output or Secret material.
        let stderr = String::from_utf8_lossy(&output.stderr);
        let reason = if stderr.contains("conflict with") || stderr.contains("Apply failed with") {
            "HelmFieldOwnershipConflict"
        } else if stderr.contains("ExplicitUpgradeRequired") {
            "HelmTransitionGuardRefused"
        } else if stderr.contains("immutable")
            && (stderr.contains("Job") || stderr.contains("jobs"))
        {
            "HelmImmutableJobConflict"
        } else if stderr.contains("cannot patch") || stderr.contains("immutable") {
            "HelmResourceUpdateFailed"
        } else {
            "HelmCommandFailed"
        };
        let action = args.first().map(String::as_str).unwrap_or("command");
        anyhow::bail!("{reason}: helm {action}; inspect status and history for this installation");
    }
    ensure!(output.stdout.len() <= 4 * 1024 * 1024, "HelmOutputTooLarge");
    Ok(output.stdout)
}
async fn helm_update_mode() -> Result<Vec<String>> {
    let version = helm_output(&["version".into(), "--short".into()]).await?;
    let version = std::str::from_utf8(&version)?.trim();
    ensure!(
        version.starts_with("v3.") || version.starts_with("v4."),
        "UnsupportedHelmVersion: use Helm 3 or 4"
    );
    // Helm 4 defaults to SSA. Quiescing and native node reconciliation update
    // fields outside Helm's apply manager, so use Helm's ordinary three-way
    // update path rather than forcing ownership conflicts or replacing volumes.
    Ok(if version.starts_with("v4.") {
        vec!["--server-side=false".into()]
    } else {
        vec![]
    })
}
async fn helm_history(name: &str, ns: &str) -> Result<Vec<Value>> {
    let mut history: Vec<Value> = serde_json::from_slice(
        &helm_output(&[
            "history".into(),
            name.into(),
            "--namespace".into(),
            ns.into(),
            "--max".into(),
            "256".into(),
            "--output".into(),
            "json".into(),
        ])
        .await?,
    )?;
    history.sort_by_key(|v| v["revision"].as_u64().unwrap_or(0));
    Ok(history)
}
fn recovery_revision(history: &[Value]) -> Result<u64> {
    history
        .iter()
        .rev()
        .find(|v| matches!(v["status"].as_str(), Some("deployed" | "superseded")))
        .context("NoRecoverableHelmRevision")?["revision"]
        .as_u64()
        .context("InvalidHelmRevision")
}
async fn verify_helm_recovery(
    context: &Installer,
    record: &Journal,
    name: &str,
    revision: u64,
) -> Result<()> {
    let bytes = helm_output(&[
        "get".into(),
        "manifest".into(),
        name.into(),
        "--namespace".into(),
        record.options.namespace.clone(),
        "--revision".into(),
        revision.to_string(),
    ])
    .await?;
    let raw = std::str::from_utf8(&bytes)?;
    let mut checked = std::collections::BTreeSet::new();
    for document in raw.split("\n---") {
        let body = document
            .lines()
            .filter(|l| !l.trim().is_empty() && !l.starts_with('#') && l.trim() != "---")
            .collect::<Vec<_>>()
            .join("\n");
        if body.is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(&body)
            .context("UnsupportedHelmRecoveryManifest: use the canonical release chart")?;
        let resource_name = value["metadata"]["name"]
            .as_str()
            .context("InvalidHelmRecoveryManifest")?;
        if value["kind"] == "Secret" {
            let current = context
                .api_at("Secret", Some(&record.options.namespace))?
                .get(resource_name)
                .await?;
            ensure!(
                current.labels().get(LABEL) == Some(&record.options.installation)
                    && current.data["data"] == value["data"],
                "HelmRecoveryCredentialDrift: reapply the current chart to record live credentials before upgrading"
            );
            checked.insert(resource_name.to_owned());
        }
        if value["kind"] == "ConfigMap"
            && resource_name == format!("{}-config", record.options.installation)
        {
            ensure!(
                value["data"]["configuration.json"] == record.configuration,
                "HelmRecoveryPolicyDrift: reapply the current chart to record live build policy before upgrading"
            );
            checked.insert(resource_name.to_owned());
        }
    }
    for suffix in [
        "registry-credentials",
        "admission-credentials",
        "builder-credentials",
        "config",
    ] {
        ensure!(
            checked.contains(&format!("{}-{suffix}", record.options.installation)),
            "IncompleteHelmRecoveryManifest"
        );
    }
    Ok(())
}
async fn helm_transition(
    context: &Installer,
    record: &Journal,
    chart: Option<&Path>,
    lease: &OperationLease,
) -> Result<()> {
    let ns = &record.options.namespace;
    let state = Api::<ConfigMap>::namespaced(context.client.clone(), ns)
        .get(&format!("{}-installation", record.options.installation))
        .await?;
    let release_name = state
        .annotations()
        .get("meta.helm.sh/release-name")
        .context("MissingHelmReleaseIdentity")?;
    let base = vec!["--namespace".into(), ns.clone()];
    let history = helm_history(release_name, ns).await?;
    let values_match = |value: &Value| -> Result<()> {
        ensure!(
            value["images"]["repository"] == record.target.repository,
            "HelmImageCatalogConflict"
        );
        for (role, component) in &record.target.components {
            ensure!(
                value["images"]["digests"][role] == component.digest,
                "HelmImageCatalogConflict"
            );
        }
        Ok(())
    };
    if let Some(current) = history.last()
        && current["status"] == "deployed"
        && current["app_version"] == record.target.release
    {
        let mut args = vec![
            "get".into(),
            "values".into(),
            release_name.clone(),
            "--all".into(),
            "--output".into(),
            "json".into(),
        ];
        args.extend(base.clone());
        values_match(&serde_json::from_slice(&helm_output(&args).await?)?)?;
        restore_helm_policy(context, record, &state, release_name, lease).await?;
        return Ok(());
    }
    // Helm permits rollback out of an interrupted pending revision. Use a real
    // recorded revision, never edit Helm's Secret history to clear its lock.
    if history
        .last()
        .and_then(|v| v["status"].as_str())
        .is_some_and(|s| s.starts_with("pending-"))
    {
        let revision = recovery_revision(&history)?;
        verify_helm_recovery(context, record, release_name, revision).await?;
        // A partial chart may already have restarted a predecessor operator.
        // Fence it again before replaying history. Readiness Jobs carry API-
        // generated immutable selectors; recreating these disposable checks
        // avoids patching historical Jobs absent from the partial manifest.
        quiesce(context, &record.options, lease).await?;
        cleanup_worker_checks(context, &[], &record.manager, lease).await?;
        let mut args = vec![
            "rollback".into(),
            release_name.clone(),
            revision.to_string(),
            "--timeout".into(),
            "10m".into(),
        ];
        args.extend(base.clone());
        args.extend(helm_update_mode().await?);
        lease.check().await?;
        // Clear Helm's pending revision without waiting for historical node
        // placement. The guarded target is rendered against current topology.
        helm_output(&args).await?;
        quiesce(context, &record.options, lease).await?;
    }
    let path = chart.context("VerifiedTargetChartRequired")?;
    // Render the target chart against current placement. Replaying an old
    // manifest would resurrect departed Nodes and discard reconciled workers.
    // Helm records this compatible reversal as a new revision.
    let mut args = vec![
        "upgrade".into(),
        release_name.clone(),
        path.to_str().context("InvalidHelmChartPath")?.into(),
        "--reuse-values".into(),
        "--wait".into(),
        "--timeout".into(),
        "10m".into(),
        "--set-string".into(),
        format!("images.repository={}", record.target.repository),
    ];
    for (role, component) in &record.target.components {
        args.extend([
            "--set-string".into(),
            format!("images.digests.{role}={}", component.digest),
        ]);
    }
    args.extend(base);
    args.extend(helm_update_mode().await?);
    lease.check().await?;
    helm_output(&args).await?;
    restore_helm_policy(context, record, &state, release_name, lease).await?;
    Ok(())
}

async fn restore_helm_policy(
    context: &Installer,
    record: &Journal,
    state: &ConfigMap,
    release_name: &str,
    lease: &OperationLease,
) -> Result<()> {
    let ns = &record.options.namespace;
    if record.target.release == "0.4.1"
        && let Some(raw) = &record.node_policy
    {
        let desired = json!({"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":format!("{}-node-policy",record.options.installation),"namespace":ns,"labels":{LABEL:record.options.installation,"app.kubernetes.io/managed-by":"Helm"},"annotations":{"kimage.dev/manager":"helm","meta.helm.sh/release-name":release_name,"meta.helm.sh/release-namespace":ns},"ownerReferences":[{"apiVersion":"v1","kind":"ConfigMap","name":state.name_any(),"uid":state.uid(),"controller":false,"blockOwnerDeletion":false}]},"data":{"policy.json":raw}});
        lease.check().await?;
        replace_owned(context, &desired, "helm").await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn direction_uses_semantic_ordering_across_minor_digits_and_prereleases() {
        assert!(!super::is_rollback("0.9.0", "0.10.0").unwrap());
        assert!(super::is_rollback("0.10.0", "0.9.0").unwrap());
        assert!(!super::is_rollback("0.5.0-rc.1", "0.5.0").unwrap());
        assert!(super::is_rollback("0.5.0", "0.5.0-rc.1").unwrap());
        assert!(!super::is_rollback("0.5.0", "1.0.0").unwrap());
        assert!(super::is_rollback("1.0.0", "0.5.0").unwrap());
    }
}

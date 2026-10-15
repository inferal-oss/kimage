use crate::api::{Image, ImageRelease};
use anyhow::{Context, Result, ensure};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::StatusCode,
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use kimage_core::{
    admission::{Policy, RESOLUTIONS},
    catalog::{Catalog, Release},
    selector::Selector,
};
use kube::{Api, Client, ResourceExt, api::ListParams};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
#[derive(Clone)]
pub struct Admission {
    pub client: Client,
    pub namespace: String,
    pub cluster_wide: bool,
    pub excluded_namespaces: BTreeSet<String>,
    pub endpoint: String,
    pub nodes: BTreeSet<String>,
    pub key_file: PathBuf,
    pub input_identity: Option<(String, String)>,
    pub builder_image: Option<String>,
    pub readiness_scope: Option<(String, String)>,
}
impl Admission {
    fn scoped(&self, request: &Value) -> Result<Self> {
        let namespace = request["namespace"]
            .as_str()
            .context("MissingAdmissionNamespace")?;
        if self.cluster_wide {
            let (system, _) = self
                .readiness_scope
                .as_ref()
                .context("ClusterAdmissionSystemScopeRequired")?;
            let scope = kimage_core::scope::NamespaceScope {
                system_namespace: system.clone(),
                application_namespace: None,
                cluster_wide: true,
                excluded_namespaces: self.excluded_namespaces.clone(),
            };
            ensure!(scope.allows(namespace), "AdmissionScopeMismatch");
        } else {
            ensure!(namespace == self.namespace, "AdmissionScopeMismatch");
        }
        let mut scoped = self.clone();
        scoped.namespace = namespace.into();
        Ok(scoped)
    }
}
async fn capture(state: &Admission, pod: &Value) -> Result<BTreeMap<String, Catalog>> {
    let images: Api<Image> = Api::namespaced(state.client.clone(), &state.namespace);
    let releases: Api<ImageRelease> = Api::namespaced(state.client.clone(), &state.namespace);
    let mut names = BTreeSet::new();
    for kind in ["containers", "initContainers", "ephemeralContainers"] {
        if let Some(items) = pod["spec"][kind].as_array() {
            for c in items {
                if let Some(uri) = c["image"].as_str().filter(|s| s.starts_with("platform://")) {
                    let selector = Selector::parse(uri)?;
                    ensure!(
                        selector.namespace == state.namespace,
                        "CrossNamespaceDenied"
                    );
                    names.insert(selector.name);
                }
            }
        }
    }
    if let Some(annotation) = pod["metadata"]["annotations"][RESOLUTIONS].as_str() {
        ensure!(annotation.len() <= 16384, "ResolutionMetadataTooLarge");
        for e in serde_json::from_str::<Vec<Value>>(annotation)? {
            names.insert(Selector::parse(e["selector"].as_str().context("InvalidSelector")?)?.name);
        }
    }
    ensure!(names.len() <= 64, "TooManyImages");
    let mut result = BTreeMap::new();
    for name in names {
        let before = images.get_opt(&name).await?;
        let live_uid = before.as_ref().and_then(ResourceExt::uid);
        let mut published = Vec::new();
        let mut continuation: Option<String> = None;
        loop {
            let mut params = ListParams::default().limit(256);
            if let Some(token) = &continuation {
                params = params.continue_token(token);
            }
            let page = releases.list(&params).await?;
            for r in page.items {
                if r.spec.image_name != name
                    || live_uid
                        .as_ref()
                        .is_some_and(|uid| &r.spec.image_uid != uid)
                    || r.metadata.deletion_timestamp.is_some()
                    || !r.status.as_ref().is_some_and(|s| {
                        s.verified && s.observed_generation == r.metadata.generation.unwrap_or(1)
                    })
                {
                    continue;
                }
                ensure!(r.spec.image_name == name, "ReleaseIdentityMismatch");
                published.push(Release {
                    namespace: state.namespace.clone(),
                    image_name: name.clone(),
                    image_uid: r.spec.image_uid.clone(),
                    uid: r.uid().context("MissingReleaseUid")?,
                    resource_version: r.resource_version().context("MissingReleaseRv")?,
                    version: r.spec.version,
                    root: r.spec.root,
                    platform: r.spec.platform,
                    snapshot: r.spec.snapshot,
                });
            }
            ensure!(published.len() <= 4096, "CatalogTooLarge");
            continuation = page.metadata.continue_.filter(|s| !s.is_empty());
            if continuation.is_none() {
                break;
            }
        }
        let after = images.get_opt(&name).await?;
        ensure!(
            before.as_ref().map(|i| (i.uid(), i.resource_version()))
                == after.as_ref().map(|i| (i.uid(), i.resource_version())),
            "CatalogChangedRetry"
        );
        let retained_uids: BTreeSet<_> = published.iter().map(|r| r.image_uid.clone()).collect();
        let uid = match live_uid {
            Some(uid) => uid,
            None => {
                ensure!(retained_uids.len() == 1, "AmbiguousOrMissingRetainedImage");
                retained_uids.into_iter().next().unwrap()
            }
        };
        let resource_version = before
            .as_ref()
            .and_then(ResourceExt::resource_version)
            .unwrap_or_else(|| "retained".into());
        let platforms = before
            .as_ref()
            .map(|i| i.spec.declared_platforms())
            .unwrap_or_default();
        let channels = before.map(|i| i.spec.channels).unwrap_or_default();
        result.insert(
            name.clone(),
            Catalog {
                namespace: state.namespace.clone(),
                name,
                image_uid: uid,
                resource_version,
                channels,
                platforms,
                releases: published,
            },
        );
    }
    Ok(result)
}
/// Only the API-authenticated publisher identity may create the fixed input
/// helper in application scope. Labels, annotations and Pod serviceAccountName
/// cannot grant this exception. The helper receives no API credentials.
fn authorized_input(request: &Value, identity: Option<&(String, String)>) -> bool {
    let Some((username, image)) = identity else {
        return false;
    };
    if request["operation"] != "CREATE" || request["userInfo"]["username"] != *username {
        return false;
    }
    let spec = &request["object"]["spec"];
    if spec["automountServiceAccountToken"] != false || spec.get("ephemeralContainers").is_some() {
        return false;
    }
    let Some(containers) = spec["containers"].as_array() else {
        return false;
    };
    let Some(init) = spec["initContainers"].as_array() else {
        return false;
    };
    !containers.is_empty()
        && !init.is_empty()
        && containers.iter().chain(init).all(|c| {
            c["image"] == *image
                && c["imagePullPolicy"] == "IfNotPresent"
                && c["command"]
                    .as_array()
                    .is_some_and(|a| a.first() == Some(&json!("/kimage-input")))
        })
}
fn authorized_session(
    request: &Value,
    identity: Option<&(String, String)>,
    builder: Option<&str>,
) -> bool {
    let (Some((username, input)), Some(builder)) = (identity, builder) else {
        return false;
    };
    let spec = &request["object"]["spec"];
    if request["operation"] != "CREATE"
        || request["userInfo"]["username"] != *username
        || spec["automountServiceAccountToken"] != false
        || spec.get("ephemeralContainers").is_some()
    {
        return false;
    }
    let (Some(containers), Some(init)) = (
        spec["containers"].as_array(),
        spec["initContainers"].as_array(),
    ) else {
        return false;
    };
    containers.len() == 1
        && init.len() == 1
        && containers[0]["image"] == builder
        && containers[0]["imagePullPolicy"] == "IfNotPresent"
        && containers[0]["command"] == json!(["/snapshot/session", "build-session"])
        && init[0]["image"] == *input
        && init[0]["imagePullPolicy"] == "IfNotPresent"
        && init[0]["command"] == json!(["/kimage-input", "prepare-build"])
}
async fn authorized_native_helper(state: &Admission, request: &Value) -> Result<bool> {
    let (Some((username, _)), Some((system, installation))) =
        (&state.input_identity, &state.readiness_scope)
    else {
        return Ok(false);
    };
    if request["operation"] != "CREATE" || request["userInfo"]["username"] != *username {
        return Ok(false);
    }
    let maps: Api<k8s_openapi::api::core::v1::ConfigMap> =
        Api::namespaced(state.client.clone(), system);
    for architecture in ["amd64", "arm64"] {
        let Some(map) = maps
            .get_opt(&format!("{installation}-builders-{architecture}"))
            .await?
        else {
            continue;
        };
        ensure!(
            map.immutable == Some(true)
                && map.labels().get(crate::install::LABEL) == Some(installation),
            "BuilderCatalogOwnershipConflict"
        );
        let catalog: Value = serde_json::from_str(
            map.data
                .as_ref()
                .and_then(|d| d.get("catalog.json"))
                .context("MissingBuilderCatalog")?,
        )?;
        let builder = &catalog["builders"]["default"];
        ensure!(
            catalog["schema"] == 1
                && builder["platforms"] == json!([format!("linux/{architecture}")]),
            "UnsupportedBuilderPlatform"
        );
        let input = builder["inputImage"]
            .as_str()
            .context("MissingNativeInputImage")?;
        let worker = builder["image"]
            .as_str()
            .context("MissingNativeBuilderImage")?;
        for (reference, role) in [(input, "input"), (worker, "builder")] {
            ensure!(
                reference
                    .strip_prefix(&format!("{}/{system}/system-{role}@", state.endpoint))
                    .is_some_and(kimage_core::oci::valid_digest),
                "InvalidNativeComponentReference"
            );
        }
        let identity = (username.clone(), input.to_owned());
        if request["object"]["spec"]["nodeName"] == builder["node"]
            && (authorized_input(request, Some(&identity))
                || authorized_session(request, Some(&identity), Some(worker)))
        {
            return Ok(true);
        }
    }
    Ok(false)
}
async fn eligible_nodes(
    state: &Admission,
) -> Result<BTreeMap<String, kimage_core::admission::NodeInfo>> {
    let Some((namespace, installation)) = &state.readiness_scope else {
        crate::metrics::nodes_ready(state.nodes.len());
        return Ok(state
            .nodes
            .iter()
            .map(|n| {
                (
                    n.clone(),
                    kimage_core::admission::NodeInfo {
                        platform: kimage_core::oci::Platform::linux_amd64(),
                        labels: BTreeMap::new(),
                    },
                )
            })
            .collect());
    };
    let pods: Api<k8s_openapi::api::core::v1::Pod> =
        Api::namespaced(state.client.clone(), namespace);
    let nodes: Api<k8s_openapi::api::core::v1::Node> = Api::all(state.client.clone());
    let mut eligible = BTreeMap::new();
    // Single-node profile. Later multi-node support uses one owned projection per node.
    for role in ["proxy", "proxy-next"] {
        let Some(proxy) = pods.get_opt(&format!("{installation}-{role}")).await? else {
            continue;
        };
        if proxy.labels().get(crate::install::LABEL) != Some(installation)
            || proxy.metadata.deletion_timestamp.is_some()
            || !proxy
                .status
                .as_ref()
                .and_then(|s| s.conditions.as_ref())
                .is_some_and(|cs| cs.iter().any(|c| c.type_ == "Ready" && c.status == "True"))
        {
            continue;
        }
        for name in &state.nodes {
            if proxy.spec.as_ref().and_then(|s| s.node_name.as_ref()) != Some(name) {
                continue;
            }
            let Some(node) = nodes.get_opt(name).await? else {
                continue;
            };
            if node.metadata.deletion_timestamp.is_some()
                || proxy.annotations().get("kimage.dev/node-uid") != node.uid().as_ref()
            {
                continue;
            }
            if node.status.as_ref().is_some_and(|s| {
                s.node_info.as_ref().is_some_and(|i| {
                    kimage_core::oci::Platform {
                        os: i.operating_system.clone(),
                        architecture: i.architecture.clone(),
                    }
                    .supported()
                }) && s
                    .conditions
                    .as_ref()
                    .is_some_and(|cs| cs.iter().any(|c| c.type_ == "Ready" && c.status == "True"))
            }) {
                let info = node
                    .status
                    .as_ref()
                    .and_then(|s| s.node_info.as_ref())
                    .unwrap();
                eligible.insert(
                    name.clone(),
                    kimage_core::admission::NodeInfo {
                        platform: kimage_core::oci::Platform {
                            os: info.operating_system.clone(),
                            architecture: info.architecture.clone(),
                        },
                        labels: node.labels().clone(),
                    },
                );
            }
        }
    }
    // Additional nodes are authorized by an administrator-owned endpoint record.
    // A Pod label alone cannot opt a new node into the delivery profile.
    let maps: Api<k8s_openapi::api::core::v1::ConfigMap> =
        Api::namespaced(state.client.clone(), namespace);
    let page = maps
        .list(
            &kube::api::ListParams::default()
                .labels(&format!(
                    "{}={},kimage.dev/role={}",
                    crate::install::LABEL,
                    installation,
                    crate::node_install::ENDPOINT_ROLE
                ))
                .limit(128),
        )
        .await?;
    ensure!(
        page.metadata
            .continue_
            .as_ref()
            .is_none_or(|s| s.is_empty()),
        "TooManyNodeEndpoints"
    );
    let port = state
        .endpoint
        .rsplit_once(':')
        .context("InvalidProxyEndpoint")?
        .1
        .parse::<u16>()?;
    for map in page {
        let record = map
            .data
            .as_ref()
            .and_then(|d| d.get("endpoint.json"))
            .context("MissingNodeEndpoint")?;
        ensure!(record.len() <= 32768, "NodeEndpointTooLarge");
        let record: crate::node_install::Endpoint =
            serde_json::from_str(record).context("InvalidNodeEndpoint")?;
        let scope = kimage_core::scope::NamespaceScope {
            system_namespace: namespace.clone(),
            application_namespace: (!state.cluster_wide).then(|| state.namespace.clone()),
            cluster_wide: state.cluster_wide,
            excluded_namespaces: state.excluded_namespaces.clone(),
        };
        record.validate_scope(installation, namespace, &scope.registry_namespaces(), port)?;
        let scope = crate::node_install::scope(installation, &record.node_uid);
        ensure!(
            map.name_any() == format!("{scope}-endpoint"),
            "NodeEndpointIdentityConflict"
        );
        let Some(node) = nodes.get_opt(&record.node).await? else {
            continue;
        };
        if node.uid().as_ref() != Some(&record.node_uid)
            || node.metadata.deletion_timestamp.is_some()
            || !node.status.as_ref().is_some_and(|s| {
                s.node_info.as_ref().is_some_and(|i| {
                    kimage_core::oci::Platform {
                        os: i.operating_system.clone(),
                        architecture: i.architecture.clone(),
                    }
                    .supported()
                }) && s
                    .conditions
                    .as_ref()
                    .is_some_and(|cs| cs.iter().any(|c| c.type_ == "Ready" && c.status == "True"))
            })
        {
            continue;
        }
        let Some(proxy) = pods.get_opt(&format!("{scope}-proxy")).await? else {
            continue;
        };
        if crate::node_install::proxy_matches(&proxy, &record, installation)
            && crate::node_install::ready(&proxy)
        {
            let info = node
                .status
                .as_ref()
                .and_then(|s| s.node_info.as_ref())
                .unwrap();
            eligible.insert(
                record.node,
                kimage_core::admission::NodeInfo {
                    platform: kimage_core::oci::Platform {
                        os: info.operating_system.clone(),
                        architecture: info.architecture.clone(),
                    },
                    labels: node.labels().clone(),
                },
            );
        }
    }
    crate::metrics::nodes_ready(eligible.len());
    Ok(eligible)
}
async fn handle(state: &Admission, request: &Value) -> Result<Vec<Value>> {
    let scoped = state.scoped(request)?;
    let state = &scoped;
    ensure!(
        request["uid"].as_str().is_some_and(|v| !v.is_empty()),
        "MissingAdmissionUid"
    );
    ensure!(
        request["namespace"] == state.namespace,
        "AdmissionScopeMismatch"
    );
    ensure!(
        request["resource"]["resource"] == "pods",
        "UnsupportedResource"
    );
    let pod = &request["object"];
    if authorized_input(request, state.input_identity.as_ref())
        || authorized_session(
            request,
            state.input_identity.as_ref(),
            state.builder_image.as_deref(),
        )
        || authorized_native_helper(state, request).await?
    {
        return Ok(Vec::new());
    }
    if request["operation"] != "CREATE"
        || request["subResource"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
    {
        for key in [RESOLUTIONS, kimage_core::admission::SIGNATURE] {
            ensure!(
                pod["metadata"]["annotations"].get(key)
                    == request["oldObject"]["metadata"]["annotations"].get(key),
                "ImmutableResolutionMetadata"
            );
        }
        for kind in ["containers", "initContainers", "ephemeralContainers"] {
            if let Some(items) = pod["spec"][kind].as_array() {
                for c in items {
                    let image = c["image"].as_str().unwrap_or("");
                    ensure!(
                        !image.starts_with("platform://"),
                        "UnsupportedPodImageUpdate"
                    );
                    if kimage_core::admission::internal_reference(image, &state.endpoint) {
                        let old = request["oldObject"]["spec"][kind]
                            .as_array()
                            .and_then(|items| items.iter().find(|old| old["name"] == c["name"]));
                        ensure!(
                            old.is_some_and(|old| old["image"] == c["image"])
                                && c["imagePullPolicy"] == "IfNotPresent",
                            "DirectInternalReferenceDenied"
                        );
                    }
                }
            }
        }
        return Ok(Vec::new());
    }
    let catalogs = capture(state, pod).await?;
    let node_info = if catalogs.is_empty() {
        BTreeMap::new()
    } else {
        eligible_nodes(state).await?
    };
    let nodes = if catalogs.is_empty() {
        state.nodes.clone()
    } else {
        node_info.keys().cloned().collect()
    };
    let key = std::fs::read(&state.key_file)?;
    let policy = Policy {
        namespace: &state.namespace,
        endpoint: &state.endpoint,
        request_uid: request["uid"].as_str().unwrap(),
        key: &key,
        nodes: &nodes,
        node_info: Some(&node_info),
    };
    match kimage_core::admission::patch(pod, &policy, &catalogs) {
        Ok(patch) => Ok(patch),
        Err(error) if error.to_string() == "NoPublishedRelease" => {
            if request_builds(state, request, &policy, &catalogs).await? {
                anyhow::bail!("ImageBuildPending");
            }
            anyhow::bail!("NoPublishedRelease")
        }
        Err(error) => Err(error),
    }
}
async fn request_builds(
    state: &Admission,
    request: &Value,
    policy: &Policy<'_>,
    catalogs: &BTreeMap<String, Catalog>,
) -> Result<bool> {
    let Some((system, installation)) = &state.readiness_scope else {
        anyhow::bail!("NoPublishedRelease");
    };
    let platforms =
        kimage_core::admission::requested_platforms(&request["object"], policy, catalogs)?;
    // Dry runs are side-effect free. The real CREATE records only bounded,
    // non-sensitive platform demand, never input data or build credentials.
    let dry_run = request["dryRun"] == true;
    let eager = if dry_run {
        false
    } else {
        crate::configuration::load(state.client.clone(), system, installation)
            .await?
            .build_policy
            == crate::configuration::BuildPolicy::Eager
    };
    let images: Api<Image> = Api::namespaced(state.client.clone(), &state.namespace);
    let mut requested = BTreeSet::new();
    let mut pending = false;
    for kind in ["containers", "initContainers"] {
        for container in request["object"]["spec"][kind]
            .as_array()
            .into_iter()
            .flatten()
        {
            let Some(uri) = container["image"]
                .as_str()
                .filter(|s| s.starts_with("platform://"))
            else {
                continue;
            };
            let selector = Selector::parse(uri)?;
            let Some(image) = images.get_opt(&selector.name).await? else {
                continue;
            };
            if image.metadata.deletion_timestamp.is_some()
                || !matches!(image.spec.source, crate::api::Source::Build(_))
            {
                continue;
            }
            let version = kimage_core::selector::exact(&image.spec.version)?;
            let matches = match &selector.selection {
                kimage_core::selector::Selection::Exact(v) => v == &version,
                kimage_core::selector::Selection::Range(range) => range.matches(&version),
                kimage_core::selector::Selection::Channel(channel) => {
                    image.spec.channels.get(channel) == Some(&image.spec.version)
                }
            };
            if !matches || !requested.insert(selector.name.clone()) {
                continue;
            }
            pending = true;
            if dry_run || eager || image.spec.explicit_platforms().is_some() {
                continue;
            }
            crate::configuration::record(
                state.client.clone(),
                system,
                installation,
                crate::configuration::Demand::for_image(&image, platforms.clone())?,
            )
            .await?;
        }
    }
    Ok(pending)
}
async fn review(State(state): State<Arc<Admission>>, Json(review): Json<Value>) -> Json<Value> {
    let request = &review["request"];
    let uid = request["uid"].as_str().unwrap_or("");
    let result = tokio::time::timeout(Duration::from_secs(4), handle(&state, request)).await;
    let response = match result {
        Ok(Ok(patch)) => {
            let mut r = json!({"uid":uid,"allowed":true});
            if !patch.is_empty() {
                r["patchType"] = json!("JSONPatch");
                r["patch"] = json!(STANDARD.encode(serde_json::to_vec(&patch).unwrap()));
            }
            r
        }
        Ok(Err(error)) if error.to_string() == "ImageBuildPending" => {
            if request["dryRun"] == true {
                json!({"uid":uid,"allowed":false,"status":{"code":403,"reason":"ImageBuildPending","message":"No matching native image release is ready. Server dry-run records no build demand. Create a real workload or use Eager policy, then retry."}})
            } else {
                json!({"uid":uid,"allowed":false,"status":{"code":429,"reason":"ImageBuildPending","message":"The requested native image platforms are being prepared. Workload controllers retry automatically; retry a direct Pod CREATE after the Image is ready.","details":{"retryAfterSeconds":5}}})
            }
        }
        failure => {
            let message = match failure {
                Ok(Err(error)) => error.to_string(),
                _ => "AdmissionTimedOut".into(),
            };
            let reason = message
                .split(':')
                .next()
                .filter(|r| {
                    !r.is_empty() && r.len() <= 64 && r.bytes().all(|b| b.is_ascii_alphanumeric())
                })
                .unwrap_or("AdmissionUnavailable");
            tracing::warn!(reason, "managed image admission denied");
            json!({"uid":uid,"allowed":false,"status":{"code":403,"reason":"KimageResolutionDenied","message":"Managed reference could not be resolved; check release, scope, endpoint and catalog readiness"}})
        }
    };
    crate::metrics::admission(response["allowed"] == true);
    Json(json!({"apiVersion":"admission.k8s.io/v1","kind":"AdmissionReview","response":response}))
}
async fn retention_review(
    State(state): State<Arc<Admission>>,
    Json(review): Json<Value>,
) -> Json<Value> {
    let request = &review["request"];
    let identity = state.input_identity.as_ref().map(|(name, _)| name.as_str());
    let result = state
        .scoped(request)
        .and_then(|scoped| crate::retention::validate(request, &scoped.namespace, identity));
    crate::metrics::admission(result.is_ok());
    let mut response = json!({"uid":request["uid"],"allowed":result.is_ok()});
    if result.is_err() {
        response["status"] = json!({"code":403,"reason":"RetainedReleasePinned","message":"Published releases have permanent alpha retention pins. Ordinary deletion and unpinning are disabled; uninstall preserves release metadata and registry data."});
    }
    Json(json!({"apiVersion":"admission.k8s.io/v1","kind":"AdmissionReview","response":response}))
}
pub async fn serve(state: Admission, cert: PathBuf, key: PathBuf) -> Result<()> {
    let images: Api<Image> = if state.cluster_wide {
        Api::all(state.client.clone())
    } else {
        Api::namespaced(state.client.clone(), &state.namespace)
    };
    images.list(&ListParams::default().limit(1)).await?;
    let releases: Api<ImageRelease> = if state.cluster_wide {
        Api::all(state.client.clone())
    } else {
        Api::namespaced(state.client.clone(), &state.namespace)
    };
    releases.list(&ListParams::default().limit(1)).await?;
    let router = Router::new()
        .route(
            "/capabilities",
            axum::routing::get(|| async { axum::Json(json!({"nodeEndpointSchema":1})) }),
        )
        .route("/mutate", post(review))
        .route("/retain", post(retention_review))
        .route("/readyz", get(|| async { StatusCode::OK }))
        .route("/metrics", get(crate::metrics::render))
        .layer(DefaultBodyLimit::max(768 * 1024))
        .with_state(Arc::new(state));
    crate::serving::tls("[::]:8443".parse()?, cert, key, router).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn dry_run_is_side_effect_free_and_only_current_build_versions_record_native_demand() {
        use axum::response::IntoResponse;
        let writes = Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
        let recorded = writes.clone();
        let router = Router::new().fallback(move |request:axum::extract::Request| {
            let recorded = recorded.clone();
            async move {
                let method = request.method().to_string();
                let path = request.uri().path().to_owned();
                let bytes = axum::body::to_bytes(request.into_body(),1024*1024).await.unwrap();
                let missing = || (StatusCode::NOT_FOUND,Json(json!({"apiVersion":"v1","kind":"Status","status":"Failure","reason":"NotFound","code":404}))).into_response();
                let value = match (method.as_str(),path.as_str()) {
                    ("GET","/apis/kimage.dev/v1alpha1/namespaces/apps/images/service") => json!({"apiVersion":"kimage.dev/v1alpha1","kind":"Image","metadata":{"name":"service","namespace":"apps","uid":"image-uid","generation":1,"resourceVersion":"1"},"spec":{"version":"1.0.0","source":{"build":{"format":"Containerfile","definition":{"inline":"FROM scratch\nCOPY message /message\n"},"execution":{"strategy":"Direct","builderRef":"default"},"inputs":{"contextPath":"/workspace/context","volumes":[{"name":"source","configMap":{"name":"source"}}],"volumeMounts":[{"name":"source","mountPath":"/workspace/context","readOnly":true}]}}}}}),
                    ("GET","/apis/kimage.dev/v1alpha1/namespaces/apps/imagereleases") => json!({"apiVersion":"kimage.dev/v1alpha1","kind":"ImageReleaseList","metadata":{},"items":[]}),
                    ("GET","/api/v1/namespaces/system/pods/dev-proxy") => json!({"apiVersion":"v1","kind":"Pod","metadata":{"name":"dev-proxy","labels":{"kimage.dev/installation":"dev"},"annotations":{"kimage.dev/node-uid":"node-uid"}},"spec":{"nodeName":"node","containers":[]},"status":{"conditions":[{"type":"Ready","status":"True"}]}}),
                    ("GET","/api/v1/namespaces/system/pods/dev-proxy-next") => return missing(),
                    ("GET","/api/v1/nodes/node") => json!({"apiVersion":"v1","kind":"Node","metadata":{"name":"node","uid":"node-uid","labels":{"kubernetes.io/os":"linux","kubernetes.io/arch":"arm64"}},"status":{"conditions":[{"type":"Ready","status":"True"}],"nodeInfo":{"architecture":"arm64","bootID":"","containerRuntimeVersion":"containerd://2.3.4","kernelVersion":"6.8","kubeProxyVersion":"","kubeletVersion":"v1.36.5+k3s1","machineID":"","operatingSystem":"linux","osImage":"fixture","systemUUID":""}}}),
                    ("GET","/api/v1/namespaces/system/configmaps") => json!({"apiVersion":"v1","kind":"ConfigMapList","metadata":{},"items":[]}),
                    ("GET","/api/v1/namespaces/system/configmaps/dev-config") => return missing(),
                    ("GET",path) if path.contains("/imagedemands/") => return missing(),
                    ("POST","/apis/kimage.dev/v1alpha1/namespaces/system/imagedemands") => {
                        let value:Value = serde_json::from_slice(&bytes).unwrap();
                        recorded.lock().unwrap().push(value.clone()); value
                    },
                    _ => panic!("unexpected admission API call: {method} {path}"),
                };
                Json(value).into_response()
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = Client::try_from(kube::Config::new(
            format!("http://{}", listener.local_addr().unwrap())
                .parse()
                .unwrap(),
        ))
        .unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let key = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(key.path(), [7; 32]).unwrap();
        let state = Arc::new(Admission {
            client,
            namespace: "apps".into(),
            cluster_wide: false,
            excluded_namespaces: BTreeSet::new(),
            endpoint: "127.0.0.1:17701".into(),
            nodes: BTreeSet::from(["node".into()]),
            key_file: key.path().into(),
            input_identity: None,
            builder_image: None,
            readiness_scope: Some(("system".into(), "dev".into())),
        });
        let mut request = json!({"request":{"uid":"review","namespace":"apps","operation":"CREATE","resource":{"resource":"pods"},"userInfo":{"username":"author"},"dryRun":true,"object":{"metadata":{},"spec":{"containers":[{"name":"app","image":"platform://apps/service:^1.0.0"}]}}}});
        for selector in [
            "platform://apps/service:^1.0.0",
            "platform://apps/service?version=%5E1.0.0",
        ] {
            request["request"]["object"]["spec"]["containers"][0]["image"] = json!(selector);
            let result = review(State(state.clone()), Json(request.clone())).await.0;
            assert_eq!(result["response"]["allowed"], false);
            assert_eq!(result["response"]["status"]["reason"], "ImageBuildPending");
            assert_eq!(result["response"]["status"]["code"], 403);
            assert!(writes.lock().unwrap().is_empty());
        }
        request["request"]["dryRun"] = json!(false);
        let result = review(State(state.clone()), Json(request.clone())).await.0;
        assert_eq!(result["response"]["status"]["code"], 429);
        let demand = writes.lock().unwrap()[0].clone();
        assert_eq!(
            demand["spec"]["platforms"],
            json!([{"os":"linux","architecture":"arm64"}])
        );
        assert_eq!(demand["spec"]["imageUid"], "image-uid");
        request["request"]["object"]["spec"]["containers"][0]["image"] =
            json!("platform://apps/service:2.0.0");
        let result = review(State(state), Json(request)).await.0;
        assert_eq!(
            result["response"]["status"]["reason"],
            "KimageResolutionDenied"
        );
        assert_eq!(writes.lock().unwrap().len(), 1);
        server.abort();
    }

    // Exercise the real admission handler and catalog capture, including
    // requests from controllers rather than only hand-written core patches.
    #[tokio::test]
    async fn workload_controllers_and_custom_templates_resolve_generated_pods() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let generation = Arc::new(AtomicUsize::new(0));
        let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
        let server_generation = generation.clone();
        let server_observed = observed.clone();
        let router = Router::new().fallback(move |request: axum::extract::Request| {
            let generation = server_generation.clone();
            let observed = server_observed.clone();
            async move {
                let method = request.method().clone();
                let path = request.uri().path().to_owned();
                observed.lock().unwrap().push((method.clone(), path.clone()));
                assert_eq!(method, axum::http::Method::GET);
                match path.as_str() {
                    "/apis/kimage.dev/v1alpha1/namespaces/ns/images/app" => (
                        StatusCode::NOT_FOUND,
                        Json(json!({"apiVersion":"v1","kind":"Status","status":"Failure","reason":"NotFound","code":404})),
                    ),
                    "/apis/kimage.dev/v1alpha1/namespaces/ns/imagereleases" => {
                        let versions = if generation.load(Ordering::SeqCst) == 0 {
                            vec!["1.2.0"]
                        } else {
                            vec!["1.2.0", "1.9.0"]
                        };
                        let releases: Vec<_> = versions.into_iter().map(|v| json!({
                            "apiVersion":"kimage.dev/v1alpha1","kind":"ImageRelease",
                            "metadata":{"name":format!("release-{v}"),"uid":v,"resourceVersion":"2","generation":1},
                            "spec":{"imageName":"app","imageUid":"image-uid","version":v,
                                "snapshot":kimage_core::oci::digest(b"snapshot"),
                                "requestDigest":kimage_core::oci::digest(b"request"),
                                "sourceGeneration":1,"publisherVersion":"0.2.0",
                                "root":kimage_core::oci::descriptor(v.as_bytes(), kimage_core::oci::MANIFEST),
                                "platform":{"os":"linux","architecture":"amd64"}},
                            "status":{"verified":true,"observedGeneration":1}
                        })).collect();
                        (StatusCode::OK, Json(json!({"apiVersion":"kimage.dev/v1alpha1","kind":"ImageReleaseList","metadata":{},"items":releases})))
                    }
                    _ => panic!("unexpected catalog request: {path}"),
                }
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let key = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(key.path(), [7; 32]).unwrap();
        let state = Admission {
            client: Client::try_from(kube::Config::new(
                format!("http://{address}").parse().unwrap(),
            ))
            .unwrap(),
            namespace: "ns".into(),
            cluster_wide: false,
            excluded_namespaces: BTreeSet::new(),
            endpoint: "127.0.0.1:17500".into(),
            nodes: BTreeSet::from(["node".into()]),
            key_file: key.path().into(),
            input_identity: None,
            builder_image: None,
            readiness_scope: None,
        };
        let workloads: Vec<Value> =
            serde_json::from_str(include_str!("../tests/fixtures/workload-consumers.json"))
                .unwrap();
        for workload in workloads {
            for selector in [
                "platform://ns/app:^1.2.0",
                "platform://ns/app?version=%5E1.2.0",
            ] {
                generation.store(0, Ordering::SeqCst);
                let template = json!({"metadata":{"labels":{"app":"example"}},"spec":{
                    "containers":[{"name":"app","image":selector},{"name":"external","image":"example.org/other:v1"}],
                    "initContainers":[{"name":"init","image":selector},{"name":"sidecar","image":selector,"restartPolicy":"Always"}]
                }});
                let mut controller =
                    json!({"apiVersion":workload["apiVersion"],"kind":workload["kind"],"spec":{}});
                let mut cursor = &mut controller;
                for field in workload["template"].as_str().unwrap().split('/').skip(1) {
                    if !cursor.is_object() {
                        *cursor = json!({});
                    }
                    cursor = cursor
                        .as_object_mut()
                        .unwrap()
                        .entry(field)
                        .or_insert(json!({}));
                }
                *cursor = template.clone();
                let unchanged = controller.clone();
                let mut pod = controller
                    .pointer(workload["template"].as_str().unwrap())
                    .unwrap()
                    .clone();
                pod["apiVersion"] = json!("v1");
                pod["kind"] = json!("Pod");
                let mut request = json!({"uid":"initial","namespace":"ns","operation":"CREATE",
                    "resource":{"group":"","version":"v1","resource":"pods"},
                    "userInfo":{"username":"system:serviceaccount:controllers:custom"},"object":pod});
                let changes: json_patch::Patch =
                    serde_json::from_value(json!(handle(&state, &request).await.unwrap())).unwrap();
                json_patch::patch(&mut request["object"], &changes).unwrap();
                let pinned = request["object"].clone();
                let expected = format!(
                    "127.0.0.1:17500/ns/image-uid@{}",
                    kimage_core::oci::digest(b"1.2.0")
                );
                for (kind, index) in [
                    ("containers", 0),
                    ("initContainers", 0),
                    ("initContainers", 1),
                ] {
                    assert_eq!(
                        pinned["spec"][kind][index]["image"], expected,
                        "{}",
                        workload["kind"]
                    );
                    assert_eq!(
                        pinned["spec"][kind][index]["imagePullPolicy"],
                        "IfNotPresent"
                    );
                }
                assert_eq!(
                    pinned["spec"]["containers"][1]["image"],
                    "example.org/other:v1"
                );
                assert_eq!(
                    pinned["spec"]["initContainers"][1]["restartPolicy"],
                    "Always"
                );
                assert!(handle(&state, &request).await.unwrap().is_empty());
                generation.store(1, Ordering::SeqCst);
                // Reinvocation keeps the first selection, while the controller's
                // next Pod resolves against the newer published catalog.
                assert!(handle(&state, &request).await.unwrap().is_empty());
                request["uid"] = json!("replacement");
                request["object"] = pod;
                let replacement: json_patch::Patch =
                    serde_json::from_value(json!(handle(&state, &request).await.unwrap())).unwrap();
                json_patch::patch(&mut request["object"], &replacement).unwrap();
                assert_eq!(
                    request["object"]["spec"]["containers"][0]["image"],
                    format!(
                        "127.0.0.1:17500/ns/image-uid@{}",
                        kimage_core::oci::digest(b"1.9.0")
                    )
                );
                assert_eq!(pinned["spec"]["containers"][0]["image"], expected);
                assert_eq!(controller, unchanged);
                assert!(!observed.lock().unwrap().is_empty());
            }
        }
        server.abort();
    }

    #[test]
    fn input_exception_requires_api_authenticated_identity_and_fixed_image() {
        let id = (
            "system:serviceaccount:system:operator".into(),
            "127.0.0.1:17000/system/system-input@sha256:fixed".into(),
        );
        let c = json!({"image":id.1,"imagePullPolicy":"IfNotPresent","command":["/kimage-input","capture"]});
        let mut r = json!({"operation":"CREATE","userInfo":{"username":id.0},"object":{"spec":{"automountServiceAccountToken":false,"containers":[c.clone()],"initContainers":[c]}}});
        assert!(authorized_input(&r, Some(&id)));
        r["userInfo"]["username"] = json!("system:serviceaccount:application:author");
        r["object"]["metadata"]["labels"]["kimage.dev/role"] = json!("input");
        assert!(!authorized_input(&r, Some(&id)));
        r["userInfo"]["username"] = json!(id.0);
        r["object"]["spec"]["containers"][0]["image"] = json!("another-internal-image");
        assert!(!authorized_input(&r, Some(&id)));
    }
}

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
pub struct Admission {
    pub client: Client,
    pub namespace: String,
    pub endpoint: String,
    pub nodes: BTreeSet<String>,
    pub key_file: PathBuf,
    pub input_identity: Option<(String, String)>,
    pub builder_image: Option<String>,
    pub readiness_scope: Option<(String, String)>,
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
        let channels = before.map(|i| i.spec.channels).unwrap_or_default();
        result.insert(
            name.clone(),
            Catalog {
                namespace: state.namespace.clone(),
                name,
                image_uid: uid,
                resource_version,
                channels,
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
async fn eligible_nodes(state: &Admission) -> Result<BTreeSet<String>> {
    let Some((namespace, installation)) = &state.readiness_scope else {
        crate::metrics::nodes_ready(state.nodes.len());
        return Ok(state.nodes.clone());
    };
    let pods: Api<k8s_openapi::api::core::v1::Pod> =
        Api::namespaced(state.client.clone(), namespace);
    let nodes: Api<k8s_openapi::api::core::v1::Node> = Api::all(state.client.clone());
    let mut eligible = BTreeSet::new();
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
                s.node_info
                    .as_ref()
                    .is_some_and(|i| i.architecture == "amd64" && i.operating_system == "linux")
                    && s.conditions.as_ref().is_some_and(|cs| {
                        cs.iter().any(|c| c.type_ == "Ready" && c.status == "True")
                    })
            }) {
                eligible.insert(name.clone());
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
        record.validate(installation, namespace, &state.namespace, port)?;
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
                s.node_info
                    .as_ref()
                    .is_some_and(|i| i.architecture == "amd64" && i.operating_system == "linux")
                    && s.conditions.as_ref().is_some_and(|cs| {
                        cs.iter().any(|c| c.type_ == "Ready" && c.status == "True")
                    })
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
            eligible.insert(record.node);
        }
    }
    crate::metrics::nodes_ready(eligible.len());
    Ok(eligible)
}
async fn handle(state: &Admission, request: &Value) -> Result<Vec<Value>> {
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
    let nodes = if catalogs.is_empty() {
        state.nodes.clone()
    } else {
        eligible_nodes(state).await?
    };
    let key = std::fs::read(&state.key_file)?;
    kimage_core::admission::patch(
        pod,
        &Policy {
            namespace: &state.namespace,
            endpoint: &state.endpoint,
            request_uid: request["uid"].as_str().unwrap(),
            key: &key,
            nodes: &nodes,
        },
        &catalogs,
    )
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
        _ => {
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
    let result = crate::retention::validate(request, &state.namespace, identity);
    crate::metrics::admission(result.is_ok());
    let mut response = json!({"uid":request["uid"],"allowed":result.is_ok()});
    if result.is_err() {
        response["status"] = json!({"code":403,"reason":"RetainedReleasePinned","message":"Published releases have permanent alpha retention pins. Ordinary deletion and unpinning are disabled; uninstall preserves release metadata and registry data."});
    }
    Json(json!({"apiVersion":"admission.k8s.io/v1","kind":"AdmissionReview","response":response}))
}
pub async fn serve(state: Admission, cert: PathBuf, key: PathBuf) -> Result<()> {
    let images: Api<Image> = Api::namespaced(state.client.clone(), &state.namespace);
    images.list(&ListParams::default().limit(1)).await?;
    let releases: Api<ImageRelease> = Api::namespaced(state.client.clone(), &state.namespace);
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

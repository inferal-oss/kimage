//! Reader compatibility is independent of the installed controller version.
//! Unknown source semantics pause one Image; stable published roots stay readable.
use crate::api::{Image, Source};
use anyhow::{Context, Result, ensure};
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition as Crd;
use kube::{
    Api, Client, ResourceExt,
    api::{ListParams, Patch, PatchParams},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub const BASELINE: u32 = 6;
pub const CAPABILITIES: &str = "kimage.dev/required-capabilities";
pub const READERS: &str = "kimage.dev/schema-readers";
pub const SUPPORTED: &[&str] = &["oci-archive-v1", "containerfile-v1", "frozen-oci-inputs-v1"];

pub fn requirements(image: &Image) -> Result<BTreeSet<String>> {
    let Some(raw) = image.annotations().get(CAPABILITIES) else {
        return Ok(BTreeSet::new());
    };
    ensure!(raw.len() <= 4096, "InvalidRequiredCapabilities");
    let list: Vec<String> = serde_json::from_str(raw).context("InvalidRequiredCapabilities")?;
    ensure!(
        list.len() <= 32
            && list.iter().all(|v| !v.is_empty()
                && v.len() <= 128
                && v.bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b".-".contains(&c))),
        "InvalidRequiredCapabilities"
    );
    let unique: BTreeSet<_> = list.iter().cloned().collect();
    ensure!(unique.len() == list.len(), "InvalidRequiredCapabilities");
    Ok(unique)
}
pub fn unsupported(image: &Image) -> Option<&'static str> {
    match requirements(image) {
        Err(_) => return Some("InvalidRequiredCapabilities"),
        Ok(v) if v.iter().any(|c| !SUPPORTED.contains(&c.as_str())) => {
            return Some("UnsupportedCapability");
        }
        _ => {}
    }
    if image.status.as_ref().is_some_and(|s| s.opaque_state) {
        return Some("UnsupportedImageState");
    }
    if !image.spec.extra.is_empty() {
        return Some("UnsupportedImageFields");
    }
    if matches!(image.spec.source, Source::Unsupported(_)) {
        return Some("UnsupportedImageSource");
    }
    None
}
pub async fn pause_reason(client: Client, image: &Image) -> Result<Option<&'static str>> {
    if let Some(reason) = unsupported(image) {
        return Ok(Some(reason));
    }
    if let Some(parent_uid) = image.labels().get("kimage.dev/platform-parent") {
        let owner = image
            .owner_references()
            .iter()
            .find(|r| {
                r.api_version == "kimage.dev/v1alpha1" && r.kind == "Image" && &r.uid == parent_uid
            })
            .context("PlatformParentIdentityConflict")?;
        let api: Api<Image> =
            Api::namespaced(client, &image.namespace().context("MissingImageNamespace")?);
        if let Some(parent) = api.get_opt(&owner.name).await? {
            ensure!(
                parent.uid().as_ref() == Some(parent_uid),
                "PlatformParentIdentityConflict"
            );
            return Ok(unsupported(&parent));
        }
    }
    Ok(None)
}
/// Merge only our condition; retain readiness, private build state and newer fields.
pub async fn pause(client: Client, image: &Image, reason: &str) -> Result<()> {
    let mut conditions = image
        .status
        .as_ref()
        .map(|s| s.conditions.clone())
        .unwrap_or_default();
    let generation = image.metadata.generation.unwrap_or(1);
    if conditions.iter().any(|c| {
        c.type_ == "BuildSupported"
            && c.status == "False"
            && c.reason == reason
            && c.observed_generation == Some(generation)
    }) {
        return Ok(());
    }
    conditions.retain(|c| c.type_ != "BuildSupported");
    let mut condition = crate::reconcile::condition(generation, false, reason);
    condition.type_ = "BuildSupported".into();
    conditions.push(condition);
    let api: Api<Image> =
        Api::namespaced(client, &image.namespace().context("MissingImageNamespace")?);
    let mut status = json!({"conditions":conditions});
    // The unchanged revision-6 schema requires observedGeneration whenever a
    // status is first created. Preserve an existing reader's readiness marker.
    if image.status.is_none() {
        status["observedGeneration"] = json!(generation);
    }
    api.patch_status(&image.name_any(),&PatchParams::default(),&Patch::Merge(json!({"metadata":{"uid":image.uid(),"resourceVersion":image.resource_version()},"status":status}))).await?;
    Ok(())
}
/// Remove only our stale build fence once this reader supports the resource.
pub async fn clear_pause(client: Client, image: &Image) -> Result<bool> {
    let Some(status) = &image.status else {
        return Ok(false);
    };
    if !status
        .conditions
        .iter()
        .any(|c| c.type_ == "BuildSupported")
    {
        return Ok(false);
    }
    let conditions: Vec<_> = status
        .conditions
        .iter()
        .filter(|c| c.type_ != "BuildSupported")
        .cloned()
        .collect();
    let api: Api<Image> =
        Api::namespaced(client, &image.namespace().context("MissingImageNamespace")?);
    api.patch_status(&image.name_any(),&PatchParams::default(),&Patch::Merge(json!({"metadata":{"uid":image.uid(),"resourceVersion":image.resource_version()},"status":{"conditions":conditions}}))).await?;
    Ok(true)
}
/// Older binaries require their shipped schema and supported resource semantics.
pub async fn legacy_target(
    client: Client,
    scope: &kimage_core::scope::NamespaceScope,
    target: &str,
) -> Result<()> {
    if kimage_core::selector::exact(target)? >= kimage_core::selector::exact("0.6.1")? {
        return Ok(());
    }
    let expected = if kimage_core::selector::exact(target)? < kimage_core::selector::exact("0.6.0")?
    {
        5
    } else {
        BASELINE
    };
    let crds: Api<Crd> = Api::all(client.clone());
    for desired in crate::schema::definitions()? {
        let current = crds.get(&desired.name_any()).await?;
        ensure!(
            current
                .annotations()
                .get("kimage.dev/schema-revision")
                .and_then(|v| v.parse::<u32>().ok())
                .is_some_and(|v| v <= expected),
            "LegacyReaderSchemaUnsupported: target predates the installed shared schema"
        );
        crate::schema::compatibility(&current, &desired)?;
    }
    let images: Api<Image> = if scope.cluster_wide {
        Api::all(client)
    } else {
        Api::namespaced(
            client,
            scope
                .application_namespace
                .as_deref()
                .context("ApplicationNamespaceRequired")?,
        )
    };
    let mut token = None;
    loop {
        let mut params = ListParams::default().limit(256);
        params.continue_token = token;
        let page = images.list(&params).await?;
        for image in page.items {
            if image.namespace().is_some_and(|n| scope.allows(&n)) {
                ensure!(
                    unsupported(&image).is_none(),
                    "LegacyReaderCapabilityUnsupported: target predates per-resource feature fencing"
                );
            }
        }
        token = page.metadata.continue_.filter(|v| !v.is_empty());
        if token.is_none() {
            break;
        }
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Readers {
    format: u32,
    baseline_revision: u32,
    minimum_reader: String,
    policy: String,
    definition_digest: String,
}
fn canonical(crd: &Crd) -> Crd {
    let mut c = crd.clone();
    if c.spec.names.list_kind.as_deref() == Some(&format!("{}List", c.spec.names.kind)) {
        c.spec.names.list_kind = None
    }
    if c.spec
        .conversion
        .as_ref()
        .is_some_and(|c| c.strategy == "None" && c.webhook.is_none())
    {
        c.spec.conversion = None
    }
    c
}
/// Only optional properties may be added. Known types, defaults, validation rules,
/// required members, API versions and storage/conversion settings remain identical.
fn no_defaults(value: &Value) -> bool {
    match value {
        Value::Object(m) => !m.contains_key("default") && m.values().all(no_defaults),
        Value::Array(a) => a.iter().all(no_defaults),
        _ => true,
    }
}
fn additive(old: &Value, new: &Value) -> bool {
    match (old,new) {
        (Value::Object(a),Value::Object(b))=>a.len()==b.len() && a.iter().all(|(k,v)|{
            let Some(n)=b.get(k) else{return false};
            if k=="properties" {
                let (Some(p),Some(q))=(v.as_object(),n.as_object()) else{return false};
                q.iter().filter(|(k,_)|!p.contains_key(*k)).all(|(_,v)|no_defaults(v)) && p.iter().all(|(k,v)|q.get(k).is_some_and(|n|additive(v,n)))
            } else if k=="enum" {
                matches!((v.as_array(),n.as_array()),(Some(a),Some(b)) if a.iter().all(|v|b.contains(v)))
            } else { additive(v,n) }
        }),
        (Value::Array(a),Value::Array(b))=>a.len()==b.len() && a.iter().zip(b).all(|(a,b)|additive(a,b)),
        _=>old==new,
    }
}
// Acceptance is limited to surfaces for which this reader retains opaque data.
// Stable identity, OCI descriptor, placement and demand formats need a new reader.
fn prepared_surface(kind: &str, path: &str) -> bool {
    let Some((_, field)) = path.split_once("/openAPIV3Schema/") else {
        return false;
    };
    match kind {
        "Image" => {
            field == "spec"
                || field.starts_with("spec/source")
                || field == "status"
                || field.starts_with("status/build")
                || field.starts_with("status/operation")
        }
        "ImageRelease" => {
            field == "spec" || field.starts_with("spec/provenance") || field == "status"
        }
        _ => false,
    }
}
fn surfaces(old: &Value, new: &Value, kind: &str, path: &str) -> bool {
    match (old, new) {
        (Value::Object(a), Value::Object(b)) => a.iter().all(|(k, v)| {
            let Some(n) = b.get(k) else { return false };
            if k == "properties" {
                let (Some(p), Some(q)) = (v.as_object(), n.as_object()) else {
                    return false;
                };
                (q.keys().all(|k| p.contains_key(k)) || prepared_surface(kind, path))
                    && p.iter().all(|(key, v)| {
                        q.get(key)
                            .is_some_and(|n| surfaces(v, n, kind, &format!("{path}/{key}")))
                    })
            } else if k == "enum" && v != n {
                prepared_surface(kind, path)
            } else {
                surfaces(v, n, kind, &format!("{path}/{k}"))
            }
        }),
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| surfaces(a, b, kind, path))
        }
        _ => true,
    }
}
pub fn declare_schema(crds: &mut [Crd]) -> Result<()> {
    if crate::schema::REVISION <= BASELINE {
        return Ok(());
    }
    for crd in crds {
        let readers = Readers {
            format: 1,
            baseline_revision: BASELINE,
            minimum_reader:
                crate::lifecycle::release_metadata()?["apiReader"]["minimumPreparedReader"]
                    .as_str()
                    .context("MissingSchemaReaderMinimum")?
                    .into(),
            policy: "additive-optional-v1".into(),
            definition_digest: kimage_core::oci::digest(&serde_json::to_vec(&canonical(crd).spec)?),
        };
        crd.metadata
            .annotations
            .get_or_insert_with(Default::default)
            .insert(READERS.into(), serde_json::to_string(&readers)?);
    }
    Ok(())
}
pub fn forward(existing: &Crd, desired: &Crd) -> Result<()> {
    let readers: Readers = serde_json::from_str(
        existing
            .annotations()
            .get(READERS)
            .context("MissingSchemaReaderContract")?,
    )
    .context("InvalidSchemaReaderContract")?;
    ensure!(
        readers.format == 1
            && readers.baseline_revision == BASELINE
            && readers.policy == "additive-optional-v1"
            && kimage_core::selector::exact(&readers.minimum_reader)?
                >= kimage_core::selector::exact("0.6.1")?
            && kimage_core::selector::exact(&readers.minimum_reader)?
                <= kimage_core::selector::exact(env!("CARGO_PKG_VERSION"))?,
        "UnsupportedSchemaReaderContract"
    );
    let actual = canonical(existing);
    let baseline = canonical(desired);
    ensure!(
        readers.definition_digest == kimage_core::oci::digest(&serde_json::to_vec(&actual.spec)?),
        "SchemaReaderDigestConflict"
    );
    ensure!(
        surfaces(
            &serde_json::to_value(&baseline.spec)?,
            &serde_json::to_value(&actual.spec)?,
            &baseline.spec.names.kind,
            ""
        ),
        "UnpreparedSchemaExtensionSurface"
    );
    ensure!(
        additive(
            &serde_json::to_value(&baseline.spec)?,
            &serde_json::to_value(&actual.spec)?
        ),
        "NonAdditiveSchemaChange"
    );
    // Reuse the existing ownership, manager and field ownership protections.
    let mut identity = existing.clone();
    identity.spec = desired.spec.clone();
    for key in ["kimage.dev/schema-revision", "kimage.dev/schema-digest"] {
        identity
            .metadata
            .annotations
            .as_mut()
            .context("SchemaOwnershipConflict")?
            .insert(
                key.into(),
                desired
                    .annotations()
                    .get(key)
                    .context("MissingBaselineSchemaIdentity")?
                    .clone(),
            );
    }
    crate::schema::compatibility_exact(&identity, desired)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn optional_only_means_no_validation_type_default_or_required_changes() {
        let old =
            json!({"type":"object","required":["name"],"properties":{"name":{"type":"string"}}});
        let mut new = old.clone();
        new["properties"]["future"] =
            json!({"type":"object","properties":{"mode":{"type":"string"}}});
        assert!(additive(&old, &new));
        let mut defaulted = new.clone();
        defaulted["properties"]["future"]["default"] = json!({"mode":"enabled"});
        assert!(!additive(&old, &defaulted));
        for key in ["required", "type", "default", "x-kubernetes-validations"] {
            let mut bad = new.clone();
            bad[key] = json!(["future"]);
            assert!(!additive(&old, &bad), "{key}");
        }
        new["properties"]["name"]["type"] = json!("integer");
        assert!(!additive(&old, &new));
    }
    fn image() -> Value {
        json!({"apiVersion":"kimage.dev/v1alpha1","kind":"Image","metadata":{"name":"service","namespace":"apps","uid":"image-uid","generation":1},"spec":{"version":"1.0.0","source":{"build":{"format":"Containerfile","definition":{"inline":"FROM scratch\n"},"execution":{"strategy":"Direct","builderRef":"default"},"inputs":{"contextPath":"/workspace/context"}}}}})
    }
    #[tokio::test]
    async fn fresh_unsupported_images_get_valid_status_without_replacing_existing_state() {
        use axum::{Json, Router, routing::patch};
        use std::sync::{Arc, Mutex};
        let captured = Arc::new(Mutex::new(Vec::<Value>::new()));
        let writes = captured.clone();
        let server = Router::new().route(
            "/apis/kimage.dev/v1alpha1/namespaces/apps/images/service/status",
            patch(move |Json(body): Json<Value>| {
                let writes = writes.clone();
                async move {
                    writes.lock().unwrap().push(body.clone());
                    let mut result = image();
                    result["status"] = body["status"].clone();
                    // Kubernetes merge-patches an existing status in the second case.
                    if result["status"].get("observedGeneration").is_none() {
                        result["status"]["observedGeneration"] = json!(1);
                    }
                    Json(result)
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move { axum::serve(listener, server).await.unwrap() });
        let client = Client::try_from(kube::Config::new(
            format!("http://{address}").parse().unwrap(),
        ))
        .unwrap();
        let fresh: Image = serde_json::from_value(image()).unwrap();
        pause(client.clone(), &fresh, "UnsupportedCapability")
            .await
            .unwrap();
        let mut published = image();
        published["status"] = json!({"observedGeneration":1,"releaseRef":"retained","conditions":[],"futureState":{"preserve":true}});
        let published: Image = serde_json::from_value(published).unwrap();
        pause(client, &published, "UnsupportedCapability")
            .await
            .unwrap();
        let patches = captured.lock().unwrap();
        assert_eq!(patches.len(), 2);
        let first: crate::api::ImageStatus =
            serde_json::from_value(patches[0]["status"].clone()).unwrap();
        assert_eq!(first.observed_generation, 1);
        assert_eq!(first.conditions[0].type_, "BuildSupported");
        assert_eq!(first.conditions[0].status, "False");
        assert!(patches[1]["status"].get("observedGeneration").is_none());
        assert!(patches[1]["status"].get("releaseRef").is_none());
        assert!(patches[1]["status"].get("futureState").is_none());
        handle.abort();
    }
    #[test]
    fn future_objects_cannot_poison_lists_or_lose_opaque_source_data() {
        let known = image();
        let mut future = known.clone();
        future["metadata"]["name"] = json!("future");
        future["spec"]["source"]["build"]["futureToolchain"] = json!({"mode":"new","data":[1,2,3]});
        let list:kube::api::ObjectList<Image>=serde_json::from_value(json!({"apiVersion":"kimage.dev/v1alpha1","kind":"ImageList","metadata":{},"items":[future.clone(),known]})).unwrap();
        assert_eq!(list.items.len(), 2);
        assert_eq!(unsupported(&list.items[0]), Some("UnsupportedImageSource"));
        assert_eq!(unsupported(&list.items[1]), None);
        assert_eq!(
            serde_json::to_value(&list.items[0]).unwrap()["spec"]["source"],
            future["spec"]["source"]
        );
        future["spec"]["futurePolicy"] = json!({"mode":"v2"});
        let decoded: Image = serde_json::from_value(future.clone()).unwrap();
        assert_eq!(unsupported(&decoded), Some("UnsupportedImageFields"));
        assert_eq!(
            serde_json::to_value(decoded).unwrap()["spec"]["futurePolicy"],
            future["spec"]["futurePolicy"]
        );
    }
    #[test]
    fn unknown_required_features_and_journals_pause_one_build_without_losing_readiness() {
        let mut raw = image();
        raw["metadata"]["annotations"][CAPABILITIES] = json!("[\"future-solver-v1\"]");
        let gated: Image = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(unsupported(&gated), Some("UnsupportedCapability"));
        raw["metadata"]["annotations"] = json!({});
        raw["status"] = json!({"observedGeneration":1,"releaseRef":"retained","conditions":[{"type":"Ready","status":"True","reason":"Published","message":"Published","lastTransitionTime":"2026-10-17T00:00:00Z"}],"operation":{"format":999,"privateFutureState":{"token":"test-only"}},"futureStatus":{"keep":true}});
        let opaque: Image = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(unsupported(&opaque), Some("UnsupportedImageState"));
        assert_eq!(
            opaque.status.as_ref().unwrap().release_ref.as_deref(),
            Some("retained")
        );
        let roundtrip = serde_json::to_value(opaque).unwrap();
        assert_eq!(roundtrip["status"]["operation"], raw["status"]["operation"]);
        assert_eq!(
            roundtrip["status"]["futureStatus"],
            raw["status"]["futureStatus"]
        );
        raw["metadata"]["annotations"][CAPABILITIES] =
            json!("[\"containerfile-v1\",\"containerfile-v1\"]");
        assert_eq!(
            unsupported(&serde_json::from_value(raw).unwrap()),
            Some("InvalidRequiredCapabilities")
        );
    }
    #[test]
    fn added_build_journal_and_execution_fields_pause_without_changing_the_schema() {
        let mut known = image();
        known["status"] = json!({"observedGeneration":1,"releaseRef":"retained","build":{"snapshotId":"attempt","ordinaryDigest":"digest","privateStateRef":"private-state","execution":{"strategy":"Direct","builder":"default","engine":"BuildKit","engineVersion":"0.33.0","adapterVersion":"0.6.1","workerUid":"worker","engineWorkerId":"engine-worker","engineOperationId":"operation","attemptId":"attempt","platform":{"os":"linux","architecture":"amd64"},"cacheMode":"Local","cachedExecutionSteps":0,"executedSteps":1}}});
        assert_eq!(
            unsupported(&serde_json::from_value(known.clone()).unwrap()),
            None
        );
        for nested in [false, true] {
            let mut future = known.clone();
            if nested {
                future["status"]["build"]["execution"]["futurePhase"] = json!({"keep":true});
            } else {
                future["status"]["build"]["journalFormat"] = json!(2);
            }
            let decoded: Image = serde_json::from_value(future.clone()).unwrap();
            assert_eq!(unsupported(&decoded), Some("UnsupportedImageState"));
            assert_eq!(
                decoded.status.as_ref().unwrap().release_ref.as_deref(),
                Some("retained")
            );
            assert_eq!(
                serde_json::to_value(decoded).unwrap()["status"]["build"],
                future["status"]["build"]
            );
        }
    }
    #[test]
    fn future_provenance_and_release_fields_roundtrip_with_stable_root_identity() {
        let raw = json!({"imageName":"service","imageUid":"uid","version":"1.0.0","snapshot":format!("sha256:{}","a".repeat(64)),"root":{"digest":format!("sha256:{}","b".repeat(64)),"mediaType":"application/vnd.oci.image.index.v1+json","size":123},"platform":{"os":"linux","architecture":"auto"},"requestDigest":"request","sourceGeneration":1,"publisherVersion":"9.0.0","provenance":{"format":"future","futureReceipt":{"keep":[1,2,3]},"execution":{"format":999}},"futureContract":{"keep":true}});
        let release: crate::api::ImageReleaseSpec = serde_json::from_value(raw.clone()).unwrap();
        assert!(release.provenance.is_none());
        assert_eq!(release.root.digest, raw["root"]["digest"].as_str().unwrap());
        assert_eq!(serde_json::to_value(release).unwrap(), raw);
    }
    fn future_schema() -> Crd {
        let mut crd = crate::schema::definitions().unwrap().remove(0);
        crd.metadata
            .annotations
            .as_mut()
            .unwrap()
            .insert("kimage.dev/schema-revision".into(), "7".into());
        let schema = crd.spec.versions[0]
            .schema
            .as_mut()
            .unwrap()
            .open_api_v3_schema
            .as_mut()
            .unwrap();
        let mut raw = serde_json::to_value(&*schema).unwrap();
        raw["properties"]["spec"]["properties"]["futurePolicy"] =
            json!({"type":"object","properties":{"mode":{"type":"string"}}});
        *schema = serde_json::from_value(raw).unwrap();
        resign(&mut crd);
        crd
    }
    fn resign(crd: &mut Crd) {
        let contract = Readers {
            format: 1,
            baseline_revision: 6,
            minimum_reader: "0.6.1".into(),
            policy: "additive-optional-v1".into(),
            definition_digest: kimage_core::oci::digest(
                &serde_json::to_vec(&canonical(crd).spec).unwrap(),
            ),
        };
        crd.metadata
            .annotations
            .as_mut()
            .unwrap()
            .insert(READERS.into(), serde_json::to_string(&contract).unwrap());
    }
    #[test]
    fn future_crd_requires_additive_proof_integrity_ownership_and_prepared_surfaces() {
        let desired = crate::schema::definitions().unwrap().remove(0);
        let future = future_schema();
        assert!(crate::schema::compatibility(&future, &desired).is_ok());
        let mut bad = future.clone();
        bad.metadata.annotations.as_mut().unwrap().remove(READERS);
        assert!(crate::schema::compatibility(&bad, &desired).is_err());
        let mut bad = future.clone();
        bad.spec.scope = "Cluster".into();
        assert!(crate::schema::compatibility(&bad, &desired).is_err());
        resign(&mut bad);
        assert!(crate::schema::compatibility(&bad, &desired).is_err());
        let mut bad = future.clone();
        bad.metadata
            .annotations
            .as_mut()
            .unwrap()
            .insert("kimage.dev/schema-manager".into(), "foreign".into());
        assert!(crate::schema::compatibility(&bad, &desired).is_err());
        let mut bad = future;
        let schema = bad.spec.versions[0]
            .schema
            .as_mut()
            .unwrap()
            .open_api_v3_schema
            .as_mut()
            .unwrap();
        let mut raw = serde_json::to_value(&*schema).unwrap();
        raw["properties"]["spec"]["properties"]["platform"]["properties"]["futurePlacementFlag"] =
            json!({"type":"string"});
        *schema = serde_json::from_value(raw).unwrap();
        resign(&mut bad);
        assert!(crate::schema::compatibility(&bad, &desired).is_err());
    }
    #[test]
    fn foundation_does_not_change_the_shipped_revision_six_schema() {
        let shipped: Vec<Crd> =
            serde_json::from_str(include_str!("../schemas/revision-6.json")).unwrap();
        assert_eq!(
            serde_json::to_value(crate::schema::definitions().unwrap()).unwrap(),
            serde_json::to_value(shipped).unwrap()
        );
    }
    #[test]
    fn independently_compiled_future_writer_schemas_accept_only_prepared_readers() {
        let future: Vec<Crd> =
            serde_json::from_str(include_str!("../tests/fixtures/additive-revision-7.json"))
                .unwrap();
        for baseline in crate::schema::definitions().unwrap() {
            let mut actual = future
                .iter()
                .find(|s| s.name_any() == baseline.name_any())
                .unwrap()
                .clone();
            assert!(
                crate::schema::compatibility(&actual, &baseline).is_ok(),
                "{}",
                baseline.name_any()
            );
            actual.spec.names.list_kind = Some(format!("{}List", actual.spec.names.kind));
            actual.spec.conversion =
                Some(serde_json::from_value(json!({"strategy":"None"})).unwrap());
            assert!(
                crate::schema::compatibility(&actual, &baseline).is_ok(),
                "API defaults changed {}",
                baseline.name_any()
            );
        }
    }
}

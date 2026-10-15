//! Private per-version build identity and execution journal. No secret hash status.
use crate::{
    api::Image,
    build_inputs::{Frozen, Reference},
    input_snapshot::Capture,
    reconcile::Config,
};
use anyhow::{Context, Result, ensure};
use k8s_openapi::{ByteString, api::core::v1::Secret};
use kimage_core::{build::ExecutionReceipt, oci::Descriptor};
use kube::{
    Api, ResourceExt,
    api::{Patch, PatchParams, PostParams},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    Captured,
    Submitted,
    Completed,
    Published,
    Failed,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    pub image_uid: String,
    pub version: String,
    pub generation: i64,
    pub snapshot_id: String,
    pub request_digest: String,
    pub fingerprint: String,
    pub frozen: Frozen,
    pub capture: Capture,
    pub worker_uid: String,
    pub phase: Phase,
    pub root: Option<Descriptor>,
    pub execution: Option<ExecutionReceipt>,
    pub published_fingerprint: Option<String>,
    pub published_references: Vec<Reference>,
    pub failed_reason: Option<String>,
}
pub fn name(image: &Image) -> Result<String> {
    Ok(format!(
        "buildstate-{}",
        &kimage_core::oci::digest(&serde_json::to_vec(&(
            image.uid().context("MissingImageUid")?,
            &image.spec.version
        ))?)[7..47]
    ))
}
pub async fn load(image: &Image, config: &Config) -> Result<Option<State>> {
    load_named(image, config, &name(image)?, Some(&image.spec.version)).await
}
pub async fn for_cleanup(image: &Image, config: &Config) -> Result<Option<State>> {
    let Some(reference) = image
        .status
        .as_ref()
        .and_then(|s| s.build.as_ref())
        .map(|s| s.private_state_ref.as_str())
    else {
        return Ok(None);
    };
    ensure!(
        reference.starts_with("buildstate-") && reference.len() == 51,
        "InvalidPrivateBuildStateRef"
    );
    load_named(image, config, reference, None).await
}
async fn load_named(
    image: &Image,
    config: &Config,
    reference: &str,
    version: Option<&str>,
) -> Result<Option<State>> {
    let api: Api<Secret> = Api::namespaced(config.client.clone(), &config.namespace);
    let Some(secret) = api.get_opt(reference).await? else {
        return Ok(None);
    };
    ensure!(
        secret
            .owner_references()
            .iter()
            .any(|o| o.kind == "Image" && Some(o.uid.clone()) == image.uid())
            && secret
                .labels()
                .get("kimage.dev/build-state")
                .is_some_and(|v| Some(v.clone()) == image.uid()),
        "PrivateBuildStateOwnershipConflict"
    );
    let data = secret
        .data
        .as_ref()
        .and_then(|d| d.get("state.json"))
        .context("MissingPrivateBuildState")?;
    ensure!(data.0.len() <= 786432, "PrivateBuildStateTooLarge");
    let state: State = serde_json::from_slice(&data.0).context("InvalidPrivateBuildState")?;
    ensure!(
        Some(state.image_uid.clone()) == image.uid()
            && version.is_none_or(|v| state.version == v)
            && reference
                == format!(
                    "buildstate-{}",
                    &kimage_core::oci::digest(&serde_json::to_vec(&(
                        state.image_uid.clone(),
                        &state.version
                    ))?)[7..47]
                )
            && state.snapshot_id.len() == 64
            && state.snapshot_id.bytes().all(|c| c.is_ascii_hexdigit()),
        "PrivateBuildStateIdentityConflict"
    );
    Ok(Some(state))
}
pub async fn save(
    image: &Image,
    config: &Config,
    state: &State,
    previous_id: Option<&str>,
) -> Result<()> {
    let images: Api<Image> = Api::namespaced(config.client.clone(), &config.namespace);
    let current = images.get(&image.name_any()).await?;
    ensure!(
        current.uid() == image.uid()
            && current.metadata.deletion_timestamp.is_none()
            && current.metadata.generation == Some(state.generation)
            && current
                .status
                .as_ref()
                .and_then(|s| s.build.as_ref())
                .is_some_and(|s| s.snapshot_id == state.snapshot_id),
        "StaleOperation"
    );
    let api: Api<Secret> = Api::namespaced(config.client.clone(), &config.namespace);
    let name = name(image)?;
    let bytes = serde_json::to_vec(state)?;
    ensure!(bytes.len() <= 786432, "PrivateBuildStateTooLarge");
    let existing = api.get_opt(&name).await?;
    if let Some(existing) = existing {
        let previous = load(image, config)
            .await?
            .context("MissingPrivateBuildState")?;
        ensure!(
            previous.snapshot_id == state.snapshot_id || previous_id == Some(&previous.snapshot_id),
            "StaleOperation"
        );
        api.patch(&name,&PatchParams::default(),&Patch::Merge(json!({"metadata":{"uid":existing.uid(),"resourceVersion":existing.resource_version()},"data":{"state.json":base64::Engine::encode(&base64::engine::general_purpose::STANDARD,bytes)}}))).await?;
    } else {
        let secret: Secret = serde_json::from_value(
            json!({"apiVersion":"v1","kind":"Secret","metadata":{"name":name,"labels":{"kimage.dev/build-state":image.uid()},"ownerReferences":[{"apiVersion":"kimage.dev/v1alpha1","kind":"Image","name":image.name_any(),"uid":image.uid(),"controller":true,"blockOwnerDeletion":false}]},"type":"Opaque"}),
        )?;
        let mut secret = secret;
        secret.data = Some(BTreeMap::from([("state.json".into(), ByteString(bytes))]));
        api.create(&PostParams::default(), &secret).await?;
    }
    Ok(())
}
pub fn fingerprint(image: &Image, capture: &Capture, references: &[Reference]) -> Result<String> {
    let identities: Vec<_> = references
        .iter()
        .map(|r| (&r.kind, &r.name, &r.uid))
        .collect();
    Ok(kimage_core::oci::digest(&serde_json::to_vec(
        &json!({"request":crate::reconcile::request_digest(image)?,"ordinary":capture.ordinary_digest,"definition":capture.definition_digest,"objects":identities}),
    )?))
}

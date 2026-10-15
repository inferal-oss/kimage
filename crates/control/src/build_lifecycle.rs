//! Image deletion waits for client termination and submitted-worker fencing.
use crate::{api::Image, reconcile::Config};
use anyhow::{Context, Result, ensure};
use k8s_openapi::api::core::v1::Pod;
use kube::{
    Api, ResourceExt,
    api::{ListParams, Patch, PatchParams},
};
use serde_json::json;
pub const FINALIZER: &str = "kimage.dev/build-operation";
pub async fn ensure(image: &Image, config: &Config) -> Result<Image> {
    if image.finalizers().iter().any(|f| f == FINALIZER) {
        return Ok(image.clone());
    }
    ensure!(
        image.metadata.deletion_timestamp.is_none(),
        "StaleOperation"
    );
    let mut finalizers = image.finalizers().to_vec();
    finalizers.push(FINALIZER.into());
    let api: Api<Image> = Api::namespaced(config.client.clone(), &config.namespace);
    Ok(api.patch(&image.name_any(),&PatchParams::default(),&Patch::Merge(json!({"metadata":{"uid":image.uid(),"resourceVersion":image.resource_version(),"finalizers":finalizers}}))).await?)
}
pub async fn finish(image: &Image, config: &Config) -> Result<()> {
    let state = crate::build_state::for_cleanup(image, config).await?;
    let pods: Api<Pod> = Api::namespaced(config.client.clone(), &config.namespace);
    let selector = format!(
        "kimage.dev/image-uid={},{}",
        image.uid().context("MissingImageUid")?,
        crate::build_inputs::SNAPSHOT_LABEL
    );
    let page = pods
        .list(&ListParams::default().labels(&selector).limit(128))
        .await?;
    ensure!(
        page.metadata
            .continue_
            .as_ref()
            .is_none_or(|s| s.is_empty()),
        "TooManyBuildClients"
    );
    for pod in page {
        ensure!(
            pod.owner_references()
                .iter()
                .any(|o| o.kind == "Image" && Some(o.uid.clone()) == image.uid()),
            "BuildClientOwnershipConflict"
        );
        let id = pod
            .annotations()
            .get("kimage.dev/build-snapshot-id")
            .context("MissingBuildSnapshotId")?;
        crate::direct::remove_client(config, id).await?;
        crate::build_inputs::cleanup(image, config, id).await?;
    }
    if let Some(state) = state
        && state.phase == crate::build_state::Phase::Submitted
    {
        let settings = config
            .direct
            .as_ref()
            .context("BuilderConfigurationRequired")?;
        crate::direct::fence(config, settings, &state.worker_uid).await?;
    }
    let images: Api<Image> = Api::namespaced(config.client.clone(), &config.namespace);
    let current = images.get(&image.name_any()).await?;
    ensure!(current.uid() == image.uid(), "StaleOperation");
    let finalizers: Vec<_> = current
        .finalizers()
        .iter()
        .filter(|f| f.as_str() != FINALIZER)
        .cloned()
        .collect();
    images.patch(&current.name_any(),&PatchParams::default(),&Patch::Merge(json!({"metadata":{"uid":current.uid(),"resourceVersion":current.resource_version(),"finalizers":finalizers}}))).await?;
    Ok(())
}

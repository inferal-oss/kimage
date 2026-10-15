//! Immutable archive publication. Operation checkpoints and reader Pods survive restart.
use crate::{
    api::{Image, ImageRelease, ImageReleaseSpec, ImageStatus, ReleaseStatus, Source},
    registry::Registry,
};
use anyhow::{Context, Result, ensure};
use k8s_openapi::{
    api::core::v1::{PersistentVolume, PersistentVolumeClaim, Pod},
    apimachinery::pkg::apis::meta::v1::{Condition, Time},
    jiff::Timestamp,
};
use kimage_core::{
    oci::{self, Limits},
    operation::{Identity, Operation, Phase},
};
use kube::{
    Api, Client, ResourceExt,
    api::{AttachParams, DeleteParams, Patch, PatchParams, PostParams},
};
use serde_json::json;
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct Config {
    pub scope: Option<kimage_core::scope::NamespaceScope>,
    pub direct: Option<crate::direct::Settings>,
    pub client: Client,
    pub namespace: String,
    pub node: String,
    pub input_image: String,
    pub registry_origin: reqwest::Url,
    pub registry_ca: PathBuf,
    pub publisher_password: PathBuf,
    pub registry_pod: Option<String>,
    pub cancel: CancellationToken,
    pub concurrency: Arc<tokio::sync::Semaphore>,
}
pub(crate) fn request_digest(image: &Image) -> Result<String> {
    let mut source = serde_json::to_value(&image.spec.source)?;
    if let Some(build) = source
        .get_mut("build")
        .and_then(serde_json::Value::as_object_mut)
    {
        build.remove("execution");
        if let Some(cache) = build
            .get_mut("cache")
            .and_then(serde_json::Value::as_object_mut)
        {
            cache.remove("mode");
        }
    }
    let mut declaration = json!({"source":source,"platform":image.spec.platform});
    if !image.spec.platforms.is_empty() {
        declaration["platforms"] = serde_json::to_value(image.spec.explicit_platforms())?;
    }
    Ok(oci::digest(&serde_json::to_vec(&declaration)?))
}
pub(crate) fn condition(generation: i64, ready: bool, reason: &str) -> Condition {
    Condition {
        type_: "Ready".into(),
        status: if ready { "True" } else { "False" }.into(),
        reason: reason.into(),
        message: reason.into(),
        observed_generation: Some(generation),
        last_transition_time: Time(Timestamp::now()),
    }
}
/// A status acknowledgment cannot overwrite a newer generation or another UID.
pub(crate) async fn checkpoint(
    config: &Config,
    original: &Image,
    mut status: ImageStatus,
) -> Result<()> {
    let api: Api<Image> = Api::namespaced(config.client.clone(), &config.namespace);
    let current = api.get(&original.name_any()).await?;
    ensure!(
        current.uid() == original.uid()
            && current.metadata.generation == original.metadata.generation
            && current.metadata.deletion_timestamp.is_none(),
        "StaleOperation"
    );
    if matches!(original.spec.source, Source::Build(_)) {
        ensure!(
            current
                .status
                .as_ref()
                .and_then(|s| s.build.as_ref())
                .map(|s| &s.snapshot_id)
                == original
                    .status
                    .as_ref()
                    .and_then(|s| s.build.as_ref())
                    .map(|s| &s.snapshot_id),
            "StaleOperation"
        );
    }
    status.observed_generation = original.metadata.generation.unwrap_or(1);
    // Export acknowledgments belong to the independent exporter. Publication
    // must preserve its latest fields even when this reconcile began earlier.
    status.exports = current
        .status
        .as_ref()
        .map(|s| s.exports.clone())
        .unwrap_or_default();
    if current.status.as_ref().is_some_and(|s| {
        s.observed_generation == status.observed_generation
            && s.release_ref.is_some()
            && s.conditions
                .iter()
                .any(|c| c.type_ == "Ready" && c.status == "True")
    }) && status
        .conditions
        .iter()
        .any(|c| c.type_ == "Ready" && c.status == "False")
    {
        anyhow::bail!("StaleOperation");
    }
    if let (Some(old), Some(new)) = (
        current.status.as_ref().and_then(|s| s.conditions.first()),
        status.conditions.first_mut(),
    ) && old.status == new.status
        && old.reason == new.reason
    {
        new.last_transition_time = old.last_transition_time.clone();
    }
    if current.status.as_ref().is_some_and(|current| {
        serde_json::to_value(current).ok() == serde_json::to_value(&status).ok()
    }) {
        return Ok(());
    }
    api.patch_status(
        &original.name_any(),
        &PatchParams::default(),
        &Patch::Merge(
            json!({"metadata":{"resourceVersion":current.resource_version()},"status":status}),
        ),
    )
    .await?;
    Ok(())
}
pub async fn reconcile(
    image: Arc<Image>,
    config: Arc<Config>,
) -> Result<kube::runtime::controller::Action> {
    let config = if let Some(scope) = &config.scope {
        let namespace = image.namespace().context("MissingImageNamespace")?;
        if !scope.allows(&namespace) {
            return Ok(kube::runtime::controller::Action::await_change());
        }
        let mut scoped = (*config).clone();
        scoped.namespace = namespace;
        Arc::new(scoped)
    } else {
        config
    };
    let waiting = crate::metrics::Waiting::begin();
    let _permit = config.concurrency.acquire().await?;
    drop(waiting);
    let _active = crate::metrics::Active::begin();
    let api: Api<Image> = Api::namespaced(config.client.clone(), &config.namespace);
    let Some(current) = api.get_opt(&image.name_any()).await? else {
        return Ok(kube::runtime::controller::Action::await_change());
    };
    if current.uid() != image.uid() || current.metadata.generation != image.metadata.generation {
        return Ok(kube::runtime::controller::Action::requeue(
            Duration::from_secs(1),
        ));
    }
    // A queued watch event may precede our latest failure-status event. Read the
    // authoritative deadline before deciding whether this attempt may execute.
    let image = Arc::new(current);
    let owns_build = image
        .finalizers()
        .iter()
        .any(|f| f == crate::build_lifecycle::FINALIZER);
    let closing = image.metadata.deletion_timestamp.is_some()
        || !matches!(image.spec.source, Source::Build(_))
        || image.spec.platform.is_automatic();
    let changed_platform = owns_build
        && !closing
        && crate::build_state::for_cleanup(&image, &config)
            .await?
            .is_some_and(|state| state.platform != image.spec.platform);
    if owns_build && (closing || changed_platform) {
        crate::build_lifecycle::finish(&image, &config).await?;
        return Ok(kube::runtime::controller::Action::requeue(
            Duration::from_secs(1),
        ));
    }
    if matches!(image.spec.source, Source::Build(_)) {
        if image.spec.platform.is_automatic() {
            return crate::platform_build::reconcile(image, config.clone()).await;
        }
        let config = Arc::new(crate::direct::native_context(&image, &config).await?);
        let image = Arc::new(crate::build_lifecycle::ensure(&image, &config).await?);
        return crate::build_reconcile::entry(image, config.clone()).await;
    }
    if image.metadata.deletion_timestamp.is_some() {
        return Ok(kube::runtime::controller::Action::await_change());
    }
    if image.status.as_ref().is_some_and(|s| {
        s.observed_generation == image.metadata.generation.unwrap_or(1)
            && s.operation
                .as_ref()
                .is_some_and(|o| o.phase == Phase::Failed)
    }) {
        cleanup_readers(&config, &image, image.metadata.generation.unwrap_or(1)).await?;
        return Ok(kube::runtime::controller::Action::await_change());
    }
    if let Some(delay) = retry_remaining(&image, unix_seconds()?) {
        // Status writes trigger the root watcher too. Honor the durable deadline
        // without writing another status and creating a self-triggered retry loop.
        return Ok(kube::runtime::controller::Action::requeue(
            Duration::from_secs(delay),
        ));
    }
    let cancellation = config.cancel.child_token();
    let monitor_cancel = cancellation.clone();
    let monitor_api = api.clone();
    let name = image.name_any();
    let uid = image.uid();
    let generation = image.metadata.generation;
    let monitor = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = monitor_cancel.cancelled() => break,
                _ = tokio::time::sleep(Duration::from_secs(2)) => {
                    match monitor_api.get_opt(&name).await {
                        Ok(Some(current)) if current.uid() == uid && current.metadata.generation == generation && current.metadata.deletion_timestamp.is_none() => {},
                        Ok(_) => { monitor_cancel.cancel(); break; },
                        Err(_) => {}, // CAS boundaries still reject stale completion.
                    }
                }
            }
        }
    });
    let mut operation_config = (*config).clone();
    operation_config.cancel = cancellation.clone();
    let outcome = publish(&image, &operation_config).await;
    cancellation.cancel();
    monitor.abort();
    match outcome {
        Ok(()) => Ok(kube::runtime::controller::Action::await_change()),
        Err(error) => {
            let text = error.to_string();
            let raw = text.split(':').next().unwrap_or("");
            let reason = if !raw.is_empty()
                && raw.len() <= 64
                && raw.bytes().all(|c| c.is_ascii_alphanumeric())
            {
                raw
            } else {
                "PublicationUnavailable"
            };
            if matches!(reason, "StaleOperation" | "OperationCancelled") {
                return Ok(kube::runtime::controller::Action::requeue(
                    Duration::from_secs(1),
                ));
            }
            let permanent = permanent_failure(reason);
            crate::metrics::failure(permanent);
            let current = api.get(&image.name_any()).await?;
            let mut status = current.status.clone().unwrap_or_default();
            if !status.operation.as_ref().is_some_and(|op| {
                op.identity.image_uid == image.uid().unwrap_or_default()
                    && op.identity.generation == image.metadata.generation.unwrap_or(1)
                    && op.identity.version == image.spec.version
            }) {
                status.operation = Some(Operation {
                    identity: Identity {
                        image_uid: image.uid().context("MissingImageUid")?,
                        generation: image.metadata.generation.unwrap_or(1),
                        version: image.spec.version.clone(),
                        snapshot: request_digest(&image)?,
                    },
                    attempt: 0,
                    phase: Phase::Capturing,
                    root: None,
                });
            }
            let op = status.operation.as_mut().expect("operation initialized");
            op.attempt = op.attempt.saturating_add(1);
            let attempt = op.attempt;
            if permanent {
                op.phase = Phase::Failed;
            }
            status.conditions = vec![condition(
                image.metadata.generation.unwrap_or(1),
                false,
                reason,
            )];
            let delay = (2_u64.saturating_pow(attempt.min(7))).min(120);
            if !permanent {
                let mut retry = condition(image.metadata.generation.unwrap_or(1), true, "Backoff");
                retry.type_ = "RetryAfter".into();
                retry.message = unix_seconds()?.saturating_add(delay).to_string();
                status.conditions.push(retry);
            }
            checkpoint(&config, &image, status).await?;
            if permanent {
                cleanup_readers(&config, &image, image.metadata.generation.unwrap_or(1)).await?;
            }
            tracing::warn!(image=%image.name_any(),uid=?image.uid(),generation=?image.metadata.generation,reason,attempt,"publication deferred");
            Ok(if permanent {
                kube::runtime::controller::Action::await_change()
            } else {
                kube::runtime::controller::Action::requeue(Duration::from_secs(delay))
            })
        }
    }
}
fn unix_seconds() -> Result<u64> {
    Ok(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs())
}
pub(crate) fn retry_remaining(image: &Image, now: u64) -> Option<u64> {
    image
        .status
        .as_ref()?
        .conditions
        .iter()
        .find(|c| {
            c.type_ == "RetryAfter"
                && c.status == "True"
                && c.observed_generation == image.metadata.generation
        })
        .and_then(|c| c.message.parse::<u64>().ok())
        .map(|deadline| deadline.saturating_sub(now).min(120))
        .filter(|delay| *delay > 0)
}
pub(crate) fn permanent_failure(reason: &str) -> bool {
    matches!(
        reason,
        "VersionConflict"
            | "UnsafeRelativePath"
            | "UnsupportedPlatform"
            | "PvcProducerHandoffRequired"
            | "InvalidRootDescriptor"
            | "InvalidClaimName"
            | "InputCaptureFailed"
            | "OperationResultConflict"
            | "RootDescriptorMismatch"
            | "WrongPlatform"
    ) || reason.starts_with("Invalid")
        || reason.starts_with("Unsupported")
        || reason.contains("DigestMismatch")
        || reason.contains("SizeMismatch")
        || reason.contains("DiffId")
        || reason.contains("DiffID")
        || reason.contains("PlatformMismatch")
        || reason.starts_with("Truncated")
        || reason == "MissingBlob"
}

async fn cleanup_readers(config: &Config, image: &Image, through_generation: i64) -> Result<()> {
    let uid = image.uid().context("MissingImageUid")?;
    let pods: Api<Pod> = Api::namespaced(config.client.clone(), &config.namespace);
    for pod in pods
        .list(&kube::api::ListParams::default().labels(&format!("kimage.dev/image-uid={uid}")))
        .await?
    {
        if !pod.name_any().starts_with("input-")
            || !pod
                .annotations()
                .get("kimage.dev/generation")
                .is_some_and(|g| g.parse::<i64>().is_ok_and(|g| g <= through_generation))
            || !pod
                .owner_references()
                .iter()
                .any(|o| o.uid == uid && o.kind == "Image" && o.controller == Some(true))
        {
            continue;
        }
        ensure!(
            pod.spec
                .as_ref()
                .is_some_and(|spec| spec.automount_service_account_token == Some(false)
                    && spec.containers.iter().all(|c| c
                        .image
                        .as_deref()
                        .is_some_and(|image| cleanup_image(image, &config.input_image))
                        && c.command.as_ref().is_some_and(|args| args
                            .first()
                            .is_some_and(|s| s == "/kimage-input")))),
            "InputCleanupOwnershipConflict"
        );
        pods.delete(
            &pod.name_any(),
            &DeleteParams {
                preconditions: Some(kube::api::Preconditions {
                    uid: pod.uid(),
                    resource_version: pod.resource_version(),
                }),
                ..Default::default()
            },
        )
        .await?;
    }
    Ok(())
}

fn cleanup_image(image: &str, current: &str) -> bool {
    // Deletion can clean an older operator's reader, without adopting or running
    // it. Admission permits reserved input references only to the operator.
    match (image.rsplit_once('@'), current.rsplit_once('@')) {
        (Some((repo, digest)), Some((approved, _))) => {
            repo == approved && oci::valid_digest(digest)
        }
        _ => false,
    }
}
/// Only obsolete, never-selectable metadata is discarded. Backend blobs remain
/// retained. DELETE's resourceVersion guard races safely against verification.
pub(crate) async fn cleanup_unacknowledged(config: &Config, image: &Image) -> Result<()> {
    let uid = image.uid().context("MissingImageUid")?;
    let generation = image.metadata.generation.unwrap_or(1);
    let api: Api<ImageRelease> = Api::namespaced(config.client.clone(), &config.namespace);
    let mut continuation: Option<String> = None;
    loop {
        let mut params = kube::api::ListParams::default().limit(256);
        if let Some(token) = &continuation {
            params = params.continue_token(token);
        }
        let page = api.list(&params).await?;
        for release in page.items {
            let obsolete_attempt = release.spec.source_generation == generation
                && image
                    .status
                    .as_ref()
                    .and_then(|s| s.build.as_ref())
                    .is_some_and(|build| {
                        release
                            .spec
                            .provenance
                            .as_ref()
                            .is_some_and(|p| p.execution.attempt_id != build.snapshot_id)
                    });
            if release.spec.image_uid != uid
                || release.spec.image_name != image.name_any()
                || (release.spec.source_generation >= generation && !obsolete_attempt)
                || release.status.as_ref().is_some_and(|s| s.verified)
            {
                continue;
            }
            if release.metadata.deletion_timestamp.is_none() {
                match api
                    .delete(
                        &release.name_any(),
                        &DeleteParams {
                            preconditions: Some(kube::api::Preconditions {
                                uid: release.uid(),
                                resource_version: release.resource_version(),
                            }),
                            ..Default::default()
                        },
                    )
                    .await
                {
                    Ok(_) => {}
                    Err(kube::Error::Api(e)) if e.code == 404 || e.code == 409 => continue,
                    Err(error) => return Err(error.into()),
                }
            }
            if let Some(current) = api.get_opt(&release.name_any()).await? {
                ensure!(
                    current.uid() == release.uid()
                        && current.metadata.deletion_timestamp.is_some()
                        && !current.status.as_ref().is_some_and(|s| s.verified),
                    "ReleaseCleanupConflict"
                );
                let finalizers: Vec<_> = current
                    .metadata
                    .finalizers
                    .clone()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|f| f != crate::retention::PIN)
                    .collect();
                api.patch(&current.name_any(), &PatchParams::default(), &Patch::Merge(json!({"metadata":{"resourceVersion":current.resource_version(),"finalizers":finalizers}}))).await?;
            }
        }
        continuation = page.metadata.continue_.filter(|s| !s.is_empty());
        if continuation.is_none() {
            break;
        }
    }
    Ok(())
}

async fn publish(image: &Image, config: &Config) -> Result<()> {
    image.spec.validate()?;
    cleanup_unacknowledged(config, image).await?;
    let uid = image.uid().context("MissingImageUid")?;
    let generation = image.metadata.generation.unwrap_or(1);
    let version = &image.spec.version;
    let release_name =
        kimage_core::catalog::platform_release_name(&uid, version, &image.spec.platform);
    let releases: Api<ImageRelease> = Api::namespaced(config.client.clone(), &config.namespace);
    let source = match &image.spec.source {
        Source::OciArchive(source) => source,
        Source::Build(_) => anyhow::bail!("BuildAdapterNotReady"),
    };
    let release_name = if image.spec.platform.is_automatic() {
        if let Some(reference) = image.status.as_ref().and_then(|s| s.release_ref.as_ref()) {
            match releases.get_opt(reference).await? {
                Some(existing)
                    if existing.spec.image_uid == uid && existing.spec.version == *version =>
                {
                    reference.clone()
                }
                _ => release_name,
            }
        } else {
            release_name
        }
    } else {
        release_name
    };
    if let Some(existing) = releases.get_opt(&release_name).await? {
        ensure!(
            existing.metadata.deletion_timestamp.is_none(),
            "ReleaseDeletionPending"
        );
        ensure!(
            existing.spec.image_uid == uid
                && existing.spec.version == *version
                && same_root(&existing.spec.root, &source.root)
                && (existing.spec.platform == image.spec.platform
                    || (image.spec.platform.is_automatic() && existing.spec.platform.supported())),
            "VersionConflict"
        );
        if !existing.status.as_ref().is_some_and(|s| s.verified) {
            let tunnel = if let Some(pod) = &config.registry_pod {
                Some(
                    crate::tunnel::Tunnel::open(
                        config.client.clone(),
                        &config.namespace,
                        pod,
                        5443,
                    )
                    .await?,
                )
            } else {
                None
            };
            let registry = Registry::new(
                config.registry_origin.clone(),
                format!("{}/{}", config.namespace, uid),
                &std::fs::read(&config.registry_ca)?,
                tunnel.as_ref().map(|t| t.address),
                "publisher".into(),
                std::fs::read_to_string(&config.publisher_password)?
                    .trim_end()
                    .into(),
            )?;
            registry
                .fetch_verified(&existing.spec.root, &existing.spec.platform, &config.cancel)
                .await?;
            let mut accepted = image.status.clone().unwrap_or_default();
            accepted.release_ref = Some(release_name.clone());
            accepted.conditions = vec![condition(generation, false, "Sealing")];
            checkpoint(config, image, accepted).await?;
            let verified = ReleaseStatus {
                verified: true,
                observed_generation: existing.metadata.generation.unwrap_or(1),
                conditions: vec![condition(
                    existing.metadata.generation.unwrap_or(1),
                    true,
                    "CompleteGraphReadback",
                )],
            };
            releases.patch_status(&release_name, &PatchParams::default(), &Patch::Merge(json!({"metadata":{"resourceVersion":existing.resource_version()},"status":verified}))).await?;
        }
        {
            let mut status = image.status.clone().unwrap_or_default();
            status.conditions = vec![condition(generation, true, "Published")];
            status.release_ref = Some(release_name);
            status.operation = None;
            cleanup_readers(config, image, generation).await?;
            return checkpoint(config, image, status).await;
        }
    }
    let pvcs: Api<PersistentVolumeClaim> =
        Api::namespaced(config.client.clone(), &config.namespace);
    let pvc = pvcs.get(&source.claim_name).await?;
    ensure!(
        pvc.status.as_ref().and_then(|s| s.phase.as_deref()) == Some("Bound"),
        "SourcePvcNotBound"
    );
    let volume_name = pvc
        .spec
        .as_ref()
        .and_then(|s| s.volume_name.as_ref())
        .context("SourceVolumeUnknown")?;
    let volumes: Api<PersistentVolume> = Api::all(config.client.clone());
    let volume = volumes.get(volume_name).await?;
    ensure!(
        volume
            .spec
            .as_ref()
            .and_then(|s| s.claim_ref.as_ref())
            .is_some_and(|reference| reference.uid == pvc.uid()
                && reference.namespace.as_deref() == Some(config.namespace.as_str())
                && reference.name.as_deref() == Some(source.claim_name.as_str())),
        "SourceVolumeBindingMismatch"
    );
    ensure_no_writers(config, &source.claim_name).await?;
    let snapshot = oci::digest(&serde_json::to_vec(
        &json!({"pvcUID":pvc.uid(),"volumeUID":volume.uid(),"path":source.path,"root":source.root,"platform":image.spec.platform}),
    )?);
    let identity = Identity {
        image_uid: uid.clone(),
        generation,
        version: version.clone(),
        snapshot: snapshot.clone(),
    };
    let operation = image
        .status
        .as_ref()
        .and_then(|s| s.operation.as_ref())
        .filter(|o| o.identity == identity)
        .cloned()
        .unwrap_or(Operation {
            identity: identity.clone(),
            attempt: 1,
            phase: Phase::Capturing,
            root: None,
        });
    let mut status = image.status.clone().unwrap_or_default();
    status.operation = Some(operation.clone());
    status.conditions = vec![condition(generation, false, "Capturing")];
    status.release_ref = None;
    checkpoint(config, image, status.clone()).await?;
    let operation_id = identity.id();
    let operation_label = &operation_id[..40];
    let reader_name = format!("input-{operation_label}");
    let pods: Api<Pod> = Api::namespaced(config.client.clone(), &config.namespace);
    for obsolete in pods
        .list(&kube::api::ListParams::default().labels(&format!("kimage.dev/image-uid={uid}")))
        .await?
    {
        if obsolete
            .annotations()
            .get("kimage.dev/generation")
            .is_some_and(|g| g.parse::<i64>().is_ok_and(|g| g < generation))
            && obsolete
                .owner_references()
                .iter()
                .any(|o| o.uid == uid && o.kind == "Image" && o.controller == Some(true))
        {
            pods.delete(
                &obsolete.name_any(),
                &DeleteParams {
                    preconditions: Some(kube::api::Preconditions {
                        uid: obsolete.uid(),
                        resource_version: obsolete.resource_version(),
                    }),
                    ..Default::default()
                },
            )
            .await?;
        }
    }
    let desired: Pod = serde_json::from_value(
        json!({"apiVersion":"v1","kind":"Pod","metadata":{"name":reader_name,"ownerReferences":[{"apiVersion":"kimage.dev/v1alpha1","kind":"Image","name":image.name_any(),"uid":uid,"controller":true,"blockOwnerDeletion":false}],"annotations":{"kimage.dev/generation":generation.to_string()},"labels":{"kimage.dev/image-uid":uid,"kimage.dev/operation":operation_label}},"spec":{"nodeName":config.node,"automountServiceAccountToken":false,"restartPolicy":"Never","securityContext":{"runAsUser":65532,"runAsGroup":65532,"seccompProfile":{"type":"RuntimeDefault"}},"initContainers":[{"name":"workspace","image":config.input_image,"imagePullPolicy":"IfNotPresent","command":["/kimage-input","prepare"],"securityContext":{"runAsUser":0,"runAsGroup":0,"allowPrivilegeEscalation":false,"readOnlyRootFilesystem":true,"capabilities":{"drop":["ALL"]}},"resources":{"requests":{"cpu":"10m","memory":"16Mi"},"limits":{"cpu":"100m","memory":"64Mi"}},"volumeMounts":[{"name":"snapshot","mountPath":"/snapshot"}]}],"containers":[{"name":"reader","image":config.input_image,"imagePullPolicy":"IfNotPresent","command":["/kimage-input","capture","--path",source.path],"securityContext":{"allowPrivilegeEscalation":false,"readOnlyRootFilesystem":true,"capabilities":{"drop":["ALL"]}},"resources":{"requests":{"cpu":"50m","memory":"64Mi"},"limits":{"cpu":"1","memory":"128Mi","ephemeral-storage":"1200Mi"}},"readinessProbe":{"exec":{"command":["/kimage-input","ready"]},"periodSeconds":1},"volumeMounts":[{"name":"source","mountPath":"/source","readOnly":true},{"name":"snapshot","mountPath":"/snapshot"}]}],"volumes":[{"name":"source","persistentVolumeClaim":{"claimName":source.claim_name,"readOnly":true}},{"name":"snapshot","emptyDir":{"sizeLimit":"1Gi"}}]}}),
    )?;
    if let Some(existing) = pods.get_opt(&reader_name).await? {
        ensure!(
            existing.metadata.labels.as_ref().is_some_and(|l| l
                .get("kimage.dev/operation")
                .is_some_and(|id| id == operation_label)),
            "InputOwnershipConflict"
        );
        ensure!(
            matches_declared(
                &serde_json::to_value(&desired.spec)?,
                &serde_json::to_value(&existing.spec)?
            ),
            "InputDefinitionConflict"
        );
    } else {
        pods.create(&PostParams::default(), &desired).await?;
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    loop {
        let reader = pods.get(&reader_name).await?;
        ensure!(
            !reader
                .status
                .as_ref()
                .and_then(|s| s.phase.as_deref())
                .is_some_and(|p| p == "Failed" || p == "Succeeded"),
            "InputCaptureFailed"
        );
        if reader
            .status
            .as_ref()
            .and_then(|s| s.conditions.as_ref())
            .is_some_and(|cs| cs.iter().any(|c| c.type_ == "Ready" && c.status == "True"))
        {
            break;
        }
        ensure!(
            tokio::time::Instant::now() < deadline,
            "InputCaptureTimeout"
        );
        tokio::select! {_=config.cancel.cancelled()=>anyhow::bail!("OperationCancelled"),_=tokio::time::sleep(Duration::from_millis(500))=>{}}
    }
    let file = tempfile::NamedTempFile::new()?;
    let mut dest = tokio::fs::File::create(file.path()).await?;
    let mut process = pods
        .exec(
            &reader_name,
            ["/kimage-input", "stream"],
            &AttachParams::default().container("reader").stderr(false),
        )
        .await?;
    let status_future = process.take_status().context("MissingInputStatus")?;
    let mut stdout = process.stdout().context("MissingInputStream")?;
    let mut buffer = [0; 65536];
    let mut count = 0_u64;
    let transfer = async {
        loop {
            let n = stdout.read(&mut buffer).await?;
            if n == 0 {
                break;
            }
            count += n as u64;
            ensure!(
                count <= Limits::default().archive_bytes,
                "InputArchiveTooLarge"
            );
            dest.write_all(&buffer[..n]).await?;
        }
        dest.sync_all().await?;
        Ok::<_, anyhow::Error>(())
    };
    tokio::select! {_=config.cancel.cancelled()=>anyhow::bail!("OperationCancelled"),r=tokio::time::timeout(Duration::from_secs(180),transfer)=>{r.context("InputTransferTimeout")??;}}
    drop(stdout);
    process.join().await?;
    let remote = status_future.await.context("MissingInputStatus")?;
    ensure!(
        remote.status.as_deref() == Some("Success"),
        "InputTransferFailed"
    );
    drop(dest);
    let current_claim = pvcs.get(&source.claim_name).await?;
    ensure!(
        current_claim.uid() == pvc.uid()
            && current_claim
                .spec
                .as_ref()
                .and_then(|s| s.volume_name.as_ref())
                == Some(volume_name)
            && volumes.get(volume_name).await?.uid() == volume.uid(),
        "SourceBindingChangedDuringCapture"
    );
    let expected = source.root.clone();
    let platform = image.spec.platform.clone();
    let path = file.path().to_owned();
    let verified = tokio::task::spawn_blocking(move || {
        oci::verify_archive(&path, &expected, &platform, Limits::default())
    })
    .await??;
    publish_artifact(
        image,
        config,
        VerifiedArtifact {
            verified,
            snapshot,
            provenance: None,
        },
        operation,
        status,
    )
    .await?;
    cleanup_readers(config, image, generation).await?;
    Ok(())
}
/// Standard verified artifacts enter publication without format or engine types.
pub(crate) struct VerifiedArtifact {
    pub verified: oci::VerifiedImage,
    pub snapshot: String,
    pub provenance: Option<crate::api::BuildProvenance>,
}
pub(crate) async fn publish_artifact(
    image: &Image,
    config: &Config,
    artifact: VerifiedArtifact,
    mut operation: Operation,
    mut status: ImageStatus,
) -> Result<()> {
    let VerifiedArtifact {
        verified,
        snapshot,
        provenance,
    } = artifact;
    let uid = image.uid().context("MissingImageUid")?;
    let generation = image.metadata.generation.unwrap_or(1);
    let version = &image.spec.version;
    let platform = if verified.platforms().len() == 1 {
        verified.platforms().first().unwrap().clone()
    } else {
        oci::Platform::automatic()
    };
    ensure!(
        verified
            .platforms()
            .is_subset(&image.spec.declared_platforms()),
        "UnsupportedPlatform"
    );
    let release_name = kimage_core::catalog::platform_release_name(&uid, version, &platform);
    let releases: Api<ImageRelease> = Api::namespaced(config.client.clone(), &config.namespace);
    let declared = request_digest(image)?;
    if let Some(previous) = &operation.root {
        ensure!(previous == verified.root(), "OperationResultConflict");
    }
    operation.root = Some(verified.root().clone());
    if matches!(operation.phase, Phase::Capturing | Phase::Executing) {
        operation.transition(Phase::Verified, &uid, generation)?;
    }
    status.operation = Some(operation.clone());
    status.conditions = vec![condition(generation, false, "Verified")];
    checkpoint(config, image, status.clone()).await?;
    operation.transition(Phase::Publishing, &uid, generation)?;
    status.operation = Some(operation.clone());
    status.conditions = vec![condition(generation, false, "Publishing")];
    checkpoint(config, image, status.clone()).await?;
    let tunnel = if let Some(pod) = &config.registry_pod {
        Some(
            crate::tunnel::Tunnel::open(config.client.clone(), &config.namespace, pod, 5443)
                .await?,
        )
    } else {
        None
    };
    let registry = Registry::new(
        config.registry_origin.clone(),
        format!("{}/{}", config.namespace, uid),
        &std::fs::read(&config.registry_ca)?,
        tunnel.as_ref().map(|t| t.address),
        "publisher".into(),
        std::fs::read_to_string(&config.publisher_password)?
            .trim_end()
            .into(),
    )?;
    let receipt = registry.publish(&verified, &config.cancel).await?;
    crate::metrics::publication_complete();
    checkpoint(config, image, status.clone()).await?;
    let candidate = ImageReleaseSpec {
        image_name: image.name_any(),
        image_uid: uid.clone(),
        version: version.clone(),
        snapshot,
        root: receipt.root().clone(),
        platform,
        request_digest: declared,
        source_generation: generation,
        publisher_version: env!("CARGO_PKG_VERSION").into(),
        provenance,
    };
    let release = if let Some(existing) = releases.get_opt(&release_name).await? {
        ensure!(same_artifact(&existing.spec, &candidate), "VersionConflict");
        existing
    } else {
        let mut resource = ImageRelease::new(&release_name, candidate.clone());
        resource.metadata.finalizers = Some(vec!["kimage.dev/retained".into()]);
        match releases.create(&PostParams::default(), &resource).await {
            Ok(r) => r,
            Err(kube::Error::Api(e)) if e.code == 409 => {
                let r = releases.get(&release_name).await?;
                ensure!(same_artifact(&r.spec, &candidate), "VersionConflict");
                r
            }
            Err(e) => return Err(e.into()),
        }
    };
    let release_status = ReleaseStatus {
        verified: true,
        observed_generation: release.metadata.generation.unwrap_or(1),
        conditions: vec![condition(
            release.metadata.generation.unwrap_or(1),
            true,
            "CompleteGraphReadback",
        )],
    };
    // Accept this version against the Image's current generation before making
    // its immutable release selectable. A later spec update is a new request.
    status.release_ref = Some(release_name.clone());
    status.conditions = vec![condition(generation, false, "Sealing")];
    checkpoint(config, image, status.clone()).await?;
    releases.patch_status(&release_name,&PatchParams::default(),&Patch::Merge(json!({"metadata":{"resourceVersion":release.resource_version()},"status":release_status}))).await?;
    operation.transition(Phase::Published, &uid, generation)?;
    status.operation = Some(operation);
    status.conditions = vec![condition(generation, true, "Published")];
    status.release_ref = Some(release_name);
    checkpoint(config, image, status).await?;
    tracing::info!(image=%image.name_any(),image_uid=uid,generation,digest=receipt.root().digest,"release published");
    Ok(())
}
/// Kubernetes fills omitted defaults; only declared fields must match. Arrays
/// keep exact lengths/order so injected containers/mounts cannot be adopted.
pub(crate) fn matches_declared(expected: &serde_json::Value, actual: &serde_json::Value) -> bool {
    match expected {
        serde_json::Value::Null => true,
        serde_json::Value::Object(m) => m.iter().all(|(k, v)| matches_declared(v, &actual[k])),
        serde_json::Value::Array(a) => actual.as_array().is_some_and(|b| {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| matches_declared(a, b))
        }),
        _ => expected == actual,
    }
}
fn same_root(a: &oci::Descriptor, b: &oci::Descriptor) -> bool {
    a.digest == b.digest && a.media_type == b.media_type && a.size == b.size
}
pub(crate) async fn ensure_no_writers(config: &Config, claim: &str) -> Result<()> {
    let api: Api<Pod> = Api::namespaced(config.client.clone(), &config.namespace);
    let mut continuation: Option<String> = None;
    let mut count = 0;
    loop {
        let mut params = kube::api::ListParams::default().limit(256);
        if let Some(token) = &continuation {
            params = params.continue_token(token);
        }
        let page = api.list(&params).await?;
        count += page.items.len();
        ensure!(count <= 4096, "UnsupportedNamespacePodCount");
        for pod in page.items {
            if matches!(
                pod.status.as_ref().and_then(|s| s.phase.as_deref()),
                Some("Succeeded" | "Failed")
            ) {
                continue;
            }
            let Some(spec) = pod.spec else {
                continue;
            };
            let writable: std::collections::BTreeSet<_> = spec
                .volumes
                .unwrap_or_default()
                .into_iter()
                .filter(|volume| {
                    volume
                        .persistent_volume_claim
                        .as_ref()
                        .is_some_and(|v| v.claim_name == claim && v.read_only != Some(true))
                })
                .map(|v| v.name)
                .collect();
            if writable.is_empty() {
                continue;
            }
            let value = serde_json::to_value(
                json!({"containers":spec.containers,"initContainers":spec.init_containers,"ephemeralContainers":spec.ephemeral_containers}),
            )?;
            for kind in ["containers", "initContainers", "ephemeralContainers"] {
                if let Some(containers) = value[kind].as_array() {
                    for container in containers {
                        if let Some(mounts) = container["volumeMounts"].as_array() {
                            ensure!(
                                !mounts.iter().any(|mount| mount["name"]
                                    .as_str()
                                    .is_some_and(|name| writable.contains(name))
                                    && mount["readOnly"] != true),
                                "PvcWriterNotQuiescent"
                            );
                        }
                    }
                }
            }
        }
        continuation = page.metadata.continue_.filter(|s| !s.is_empty());
        if continuation.is_none() {
            break;
        }
    }
    Ok(())
}
pub(crate) fn same_artifact(a: &ImageReleaseSpec, b: &ImageReleaseSpec) -> bool {
    a.image_uid == b.image_uid
        && a.version == b.version
        && a.platform == b.platform
        && same_root(&a.root, &b.root)
}
#[cfg(test)]
mod tests {
    #[test]
    fn verifier_platform_failure_is_permanent_but_endpoint_and_writer_failures_retry() {
        let mut root: super::oci::Descriptor = serde_json::from_value(serde_json::json!({
            "mediaType":super::oci::MANIFEST,"digest":super::oci::digest(b"root"),"size":1
        }))
        .unwrap();
        root.platform = Some(super::oci::Platform {
            os: "linux".into(),
            architecture: "arm64".into(),
        });
        let reason = super::oci::verify_archive(
            std::path::Path::new("unused"),
            &root,
            &super::oci::Platform::linux_amd64(),
            super::oci::Limits::default(),
        )
        .err()
        .unwrap()
        .to_string();
        assert_eq!(reason, "WrongPlatform");
        assert!(super::permanent_failure(&reason));
        assert!(!super::permanent_failure("RegistryUnavailable"));
        assert!(!super::permanent_failure("PvcWriterNotQuiescent"));
    }
    use super::*;
    #[test]
    fn export_configuration_never_changes_build_identity() {
        let mut image:Image=serde_json::from_value(json!({"apiVersion":"kimage.dev/v1alpha1","kind":"Image","metadata":{"name":"app","generation":1},"spec":{"version":"1.0.0","source":{"ociArchive":{"claimName":"source","path":"archive","producerHandoff":true,"root":oci::descriptor(b"root",oci::MANIFEST)}},"platform":oci::Platform::linux_amd64()}})).unwrap();
        let before = request_digest(&image).unwrap();
        image.spec.exports=serde_json::from_value(json!([{"name":"destination","registry":{"repository":"registry.example/team/api","tags":["1.0.0"],"credentialsSecretRef":{"name":"push"}}}])).unwrap();
        image.metadata.generation = Some(2);
        assert_eq!(before, request_digest(&image).unwrap());
        image.spec.exports[0].registry.tags = vec!["another-explicit-tag".into()];
        assert_eq!(before, request_digest(&image).unwrap());
    }
    #[test]
    fn status_feedback_and_restart_obey_deadline_but_new_generation_does_not() {
        let mut image: Image = serde_json::from_value(json!({"apiVersion":"kimage.dev/v1alpha1","kind":"Image","metadata":{"name":"app","generation":1},"spec":{"version":"1.0.0","source":{"ociArchive":{"claimName":"source","path":"archive","producerHandoff":true,"root":oci::descriptor(b"root",oci::MANIFEST)}},"platform":oci::Platform::linux_amd64()},"status":{"observedGeneration":1,"conditions":[{"type":"RetryAfter","status":"True","observedGeneration":1,"reason":"Backoff","message":"120","lastTransitionTime":"2026-10-14T00:00:00Z"}]}})).unwrap();
        assert_eq!(retry_remaining(&image, 100), Some(20));
        let restarted: Image =
            serde_json::from_slice(&serde_json::to_vec(&image).unwrap()).unwrap();
        assert_eq!(retry_remaining(&restarted, 110), Some(10));
        assert_eq!(retry_remaining(&image, 120), None);
        image.metadata.generation = Some(2);
        assert_eq!(retry_remaining(&image, 100), None);
    }
    #[test]
    fn cleanup_accepts_previous_helper_digest_but_not_other_repositories() {
        let current = format!(
            "127.0.0.1:17641/system/system-input@{}",
            oci::digest(b"current")
        );
        let previous = format!(
            "127.0.0.1:17641/system/system-input@{}",
            oci::digest(b"previous")
        );
        assert!(cleanup_image(&previous, &current));
        for image in [
            previous.replace("system-input", "system-operator"),
            previous.replace("/system/", "/other/"),
            previous.replace("17641", "17642"),
            previous.replace("sha256:", "sha512:"),
        ] {
            assert!(!cleanup_image(&image, &current));
        }
    }
    #[test]
    fn idempotent_archive_content_keeps_first_provenance() {
        let first = ImageReleaseSpec {
            image_name: "app".into(),
            image_uid: "uid".into(),
            version: "1.0.0".into(),
            snapshot: oci::digest(b"first capture"),
            root: oci::descriptor(b"manifest", oci::MANIFEST),
            platform: oci::Platform::linux_amd64(),
            request_digest: oci::digest(b"first request"),
            source_generation: 1,
            publisher_version: "0.1.0".into(),
            provenance: None,
        };
        let mut retry = first.clone();
        retry.source_generation = 2;
        retry.snapshot = oci::digest(b"new capture location");
        retry.request_digest = oci::digest(b"new request");
        assert!(same_artifact(&first, &retry));
        retry.root = oci::descriptor(b"other manifest", oci::MANIFEST);
        assert!(!same_artifact(&first, &retry));
        retry = first.clone();
        retry.image_uid = "recreated".into();
        assert!(!same_artifact(&first, &retry));
    }
}

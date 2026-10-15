//! Direct builds retain private input identity and reuse standard publication.
use crate::{
    api::{BuildProvenance, BuildStatus, Image, ImageRelease, Source},
    build_inputs::{self, Frozen},
    build_state::{self, Phase, State},
    direct::{self, Settings, WorkerLease},
    direct_client::{self, Record},
    input_snapshot::Capture,
    reconcile::{Config, VerifiedArtifact, checkpoint, condition},
};
use anyhow::{Context, Result, ensure};
use k8s_openapi::api::core::v1::{ConfigMap, PersistentVolumeClaim, Pod, Secret};
use kimage_core::{
    build::ExecutionReceipt,
    oci,
    operation::{Identity, Operation, Phase as OperationPhase},
};
use kube::{
    Api, ResourceExt,
    api::{AttachParams, Patch, PatchParams},
};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

fn nonce() -> Result<String> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| anyhow::anyhow!("OperationIdentityUnavailable"))?;
    Ok(hex::encode(bytes))
}
fn generation(image: &Image) -> i64 {
    image.metadata.generation.unwrap_or(1)
}
fn permanent(reason: &str) -> bool {
    crate::reconcile::permanent_failure(reason)
        || matches!(
            reason,
            "VersionConflict"
                | "ContainerfileSolveFailed"
                | "ExternalFrontendDirectiveDenied"
                | "InvalidDefinitionText"
                | "DefinitionMustBeUtf8"
                | "BuildProgressLimit"
                | "BuildProgressLineLimit"
                | "BuildExportTooLarge"
        )
        || reason.starts_with("Invalid")
        || reason.starts_with("Unsupported")
        || reason.starts_with("MissingInput")
        || reason.contains("Symlink")
        || reason == "InputVolumeSourceExclusive"
}
fn reason(error: &anyhow::Error) -> String {
    if let Some(kube::Error::Api(api)) = error.downcast_ref::<kube::Error>() {
        if api.code == 403 {
            return "BuildPermissionOrAdmissionDenied".into();
        }
        if api.code == 409 {
            return "StaleOperation".into();
        }
    }
    let text = error.to_string();
    let r = text.split(':').next().unwrap_or("");
    if !r.is_empty() && r.len() <= 64 && r.bytes().all(|b| b.is_ascii_alphanumeric()) {
        r.into()
    } else {
        "BuildUnavailable".into()
    }
}
async fn refs_unchanged_or_gone(
    config: &Config,
    references: &[build_inputs::Reference],
    check_pvc: bool,
) -> Result<bool> {
    for r in references {
        let metadata = match r.kind.as_str() {
            "ConfigMap" => {
                let api: Api<ConfigMap> = Api::namespaced(config.client.clone(), &config.namespace);
                api.get_opt(&r.name)
                    .await?
                    .map(|o| (o.uid(), o.resource_version()))
            }
            "Secret" => {
                let api: Api<Secret> = Api::namespaced(config.client.clone(), &config.namespace);
                api.get_opt(&r.name)
                    .await?
                    .map(|o| (o.uid(), o.resource_version()))
            }
            "PersistentVolumeClaim" => {
                if !check_pvc {
                    continue;
                }
                let api: Api<PersistentVolumeClaim> =
                    Api::namespaced(config.client.clone(), &config.namespace);
                if api.get_opt(&r.name).await?.is_some() {
                    return Ok(false);
                }
                None
            }
            "PersistentVolume" => continue,
            _ => return Ok(false),
        };
        if let Some((uid, rv)) = metadata
            && (uid.as_deref() != Some(&r.uid) || rv.as_deref() != Some(&r.resource_version))
        {
            return Ok(false);
        }
    }
    Ok(true)
}
async fn retained(image: &Image, config: &Config, release: &ImageRelease) -> Result<()> {
    let mut status = image.status.clone().unwrap_or_default();
    status.operation = None;
    status.release_ref = Some(release.name_any());
    status.conditions = vec![condition(generation(image), true, "Published")];
    if let Some(build) = status.build.as_mut() {
        build.ordinary_digest = release.spec.snapshot.clone();
        build.execution = release
            .spec
            .provenance
            .as_ref()
            .map(|p| p.execution.clone());
    }
    checkpoint(config, image, status).await
}
async fn adopt(
    image: &Image,
    config: &Config,
    id: &str,
    digest: &str,
    ready_reason: &str,
) -> Result<Image> {
    let mut status = image.status.clone().unwrap_or_default();
    status.observed_generation = generation(image);
    status.build = Some(BuildStatus {
        snapshot_id: id.into(),
        ordinary_digest: digest.into(),
        private_state_ref: build_state::name(image)?,
        execution: None,
    });
    status.release_ref = None;
    status.operation = Some(Operation {
        identity: Identity {
            image_uid: image.uid().context("MissingImageUid")?,
            generation: generation(image),
            version: image.spec.version.clone(),
            snapshot: digest.into(),
        },
        attempt: image
            .status
            .as_ref()
            .and_then(|s| s.operation.as_ref())
            .filter(|op| {
                op.identity.generation == generation(image) && op.identity.snapshot == digest
            })
            .map(|op| op.attempt.saturating_add(1))
            .unwrap_or(1),
        phase: OperationPhase::Capturing,
        root: None,
    });
    status.conditions = vec![condition(generation(image), false, ready_reason)];
    let api: Api<Image> = Api::namespaced(config.client.clone(), &config.namespace);
    Ok(api.patch_status(&image.name_any(),&PatchParams::default(),&Patch::Merge(json!({"metadata":{"uid":image.uid(),"resourceVersion":image.resource_version()},"status":status}))).await?)
}
async fn stream(
    config: &Config,
    id: &str,
    cancel: &CancellationToken,
) -> Result<oci::VerifiedImage> {
    let pods: Api<Pod> = Api::namespaced(config.client.clone(), &config.namespace);
    let mut process = pods
        .exec(
            &direct::client_name(id)?,
            ["/snapshot/session", "build-stream"],
            &AttachParams::default().container("client").stderr(false),
        )
        .await?;
    let remote = process.take_status().context("MissingBuildClientStatus")?;
    let mut output = process.stdout().context("MissingBuildExportStream")?;
    let file = tempfile::NamedTempFile::new()?;
    let mut destination = tokio::fs::File::create(file.path()).await?;
    let mut bytes = 0_u64;
    let transfer = async {
        let mut buffer = [0_u8; 65536];
        loop {
            let n = output.read(&mut buffer).await?;
            if n == 0 {
                break;
            }
            bytes += n as u64;
            crate::metrics::transferred(crate::metrics::Stage::Build, n);
            ensure!(bytes <= 1024 * 1024 * 1024, "BuildExportTooLarge");
            destination.write_all(&buffer[..n]).await?;
        }
        destination.sync_all().await?;
        Ok::<(), anyhow::Error>(())
    };
    tokio::select! {_=cancel.cancelled()=>anyhow::bail!("OperationCancelled"),result=tokio::time::timeout(Duration::from_secs(180),transfer)=>{result.context("BuildExportDeadline")??;}}
    drop(output);
    process.join().await?;
    ensure!(
        remote
            .await
            .context("MissingBuildClientStatus")?
            .status
            .as_deref()
            == Some("Success"),
        "BuildExportTransferFailed"
    );
    drop(destination);
    let path = file.path().to_owned();
    tokio::task::spawn_blocking(move || {
        oci::verify_export_archive(&path, &oci::Platform::linux_amd64(), oci::Limits::default())
    })
    .await?
}
async fn phase(config: &Config, id: &str) -> Result<Value> {
    let pods: Api<Pod> = Api::namespaced(config.client.clone(), &config.namespace);
    let pod = pods.get(&direct::client_name(id)?).await?;
    ensure!(
        !matches!(
            pod.status.as_ref().and_then(|s| s.phase.as_deref()),
            Some("Failed" | "Succeeded")
        ),
        "BuildClientStopped"
    );
    let running = pod
        .status
        .as_ref()
        .and_then(|s| s.container_statuses.as_ref())
        .is_some_and(|s| {
            s.iter().any(|c| {
                c.name == "client" && c.state.as_ref().and_then(|s| s.running.as_ref()).is_some()
            })
        });
    if !running {
        return Ok(json!({"phase":"Capturing"}));
    }
    serde_json::from_slice(&direct_client::exec(config, id, vec!["build-phase".into()]).await?)
        .context("InvalidBuildPhase")
}
async fn wait(
    config: &Config,
    id: &str,
    expected: &str,
    cancel: &CancellationToken,
    seconds: u64,
) -> Result<Value> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(seconds);
    loop {
        let value = tokio::select! {_=cancel.cancelled()=>anyhow::bail!("OperationCancelled"),value=phase(config,id)=>value?};
        let state = value["phase"].as_str().context("InvalidBuildPhase")?;
        if state == expected
            || (expected == "Captured" && matches!(state, "Submitted" | "Completed"))
        {
            return Ok(value);
        }
        if let Some(reason) = state.strip_prefix("Failed:") {
            ensure!(
                !reason.is_empty()
                    && reason.len() <= 64
                    && reason.bytes().all(|c| c.is_ascii_alphanumeric()),
                "InvalidBuildFailure"
            );
            anyhow::bail!("{reason}");
        }
        ensure!(
            tokio::time::Instant::now() < deadline,
            "BuildDeadlineExceeded"
        );
        tokio::select! {_=cancel.cancelled()=>anyhow::bail!("OperationCancelled"),_=tokio::time::sleep(Duration::from_secs(1))=>{}}
    }
}
fn monitor(
    image: &Image,
    config: &Config,
    id: &str,
    frozen: Frozen,
    worker_uid: String,
    lost: CancellationToken,
    cancel: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    let images: Api<Image> = Api::namespaced(config.client.clone(), &config.namespace);
    let name = image.name_any();
    let uid = image.uid();
    let generation = generation(image);
    let id = id.to_owned();
    let config = config.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {_=cancel.cancelled()=>break,_=lost.cancelled()=>{cancel.cancel();break;},_=config.cancel.cancelled()=>{cancel.cancel();break;},_=tokio::time::sleep(Duration::from_secs(1))=>{}}
            let current = images.get_opt(&name).await;
            if !matches!(current,Ok(Some(ref current)) if current.uid()==uid &&current.metadata.generation==Some(generation) &&current.metadata.deletion_timestamp.is_none() &&current.status.as_ref().and_then(|s|s.build.as_ref()).is_some_and(|s|s.snapshot_id==id))
                || build_inputs::unchanged(&config, &frozen).await.is_err()
                || !direct::worker_matches(&config, &worker_uid)
                    .await
                    .unwrap_or(false)
            {
                cancel.cancel();
                break;
            }
        }
    })
}
async fn registry(
    config: &Config,
    uid: &str,
) -> Result<(crate::registry::Registry, Option<crate::tunnel::Tunnel>)> {
    let tunnel = if let Some(pod) = &config.registry_pod {
        Some(
            crate::tunnel::Tunnel::open(config.client.clone(), &config.namespace, pod, 5443)
                .await?,
        )
    } else {
        None
    };
    let registry = crate::registry::Registry::new(
        config.registry_origin.clone(),
        format!("{}/{uid}", config.namespace),
        &std::fs::read(&config.registry_ca)?,
        tunnel.as_ref().map(|t| t.address),
        "publisher".into(),
        std::fs::read_to_string(&config.publisher_password)?
            .trim_end()
            .into(),
    )?;
    Ok((registry, tunnel))
}
async fn recover_candidate(
    image: &Image,
    config: &Config,
    release: &ImageRelease,
    state: Option<&mut State>,
) -> Result<bool> {
    let Some(provenance) = release.spec.provenance.as_ref() else {
        return Ok(false);
    };
    let id = &provenance.execution.attempt_id;
    if release.spec.source_generation != generation(image)
        || release.spec.request_digest != crate::reconcile::request_digest(image)?
        || !image
            .status
            .as_ref()
            .and_then(|s| s.build.as_ref())
            .is_some_and(|b| b.snapshot_id == *id)
        || !image.status.as_ref().is_some_and(|s| {
            s.observed_generation == generation(image)
                && s.operation.as_ref().is_some_and(|op| {
                    op.identity.generation == generation(image)
                        && Some(op.identity.image_uid.clone()) == image.uid()
                        && op.identity.version == image.spec.version
                        && op.identity.snapshot == release.spec.snapshot
                        && op.root.as_ref() == Some(&release.spec.root)
                        && matches!(
                            op.phase,
                            OperationPhase::Verified
                                | OperationPhase::Publishing
                                | OperationPhase::Published
                        )
                })
        })
    {
        return Ok(false);
    }
    let references: Vec<_> = provenance
        .inputs
        .iter()
        .map(|r| build_inputs::Reference {
            kind: r.kind.clone(),
            name: r.name.clone(),
            uid: r.uid.clone(),
            resource_version: r.resource_version.clone(),
        })
        .collect();
    if !refs_unchanged_or_gone(config, &references, false).await? {
        return Ok(false);
    }
    let (registry, _tunnel) = registry(config, &image.uid().context("MissingImageUid")?).await?;
    let verified = registry
        .fetch_verified(&release.spec.root, &image.spec.platform, &config.cancel)
        .await?;
    let mut status = image.status.clone().unwrap_or_default();
    let operation = Operation {
        identity: Identity {
            image_uid: image.uid().unwrap(),
            generation: generation(image),
            version: image.spec.version.clone(),
            snapshot: release.spec.snapshot.clone(),
        },
        attempt: 1,
        phase: OperationPhase::Verified,
        root: Some(release.spec.root.clone()),
    };
    status.operation = Some(operation.clone());
    crate::reconcile::publish_artifact(
        image,
        config,
        VerifiedArtifact {
            verified,
            snapshot: release.spec.snapshot.clone(),
            provenance: Some(provenance.clone()),
        },
        operation,
        status,
    )
    .await?;
    if let Some(state) = state {
        state.phase = Phase::Published;
        state.published_fingerprint = Some(state.fingerprint.clone());
        state.published_references = state.frozen.references.clone();
        build_state::save(image, config, state, Some(&state.snapshot_id)).await?;
    }
    direct::remove_client(config, id).await?;
    build_inputs::cleanup(image, config, id).await?;
    Ok(true)
}
async fn fail_completed(
    image: &Image,
    config: &Config,
    state: &mut State,
    reason: &str,
) -> Result<()> {
    direct::remove_client(config, &state.snapshot_id).await?;
    build_inputs::cleanup(image, config, &state.snapshot_id).await?;
    state.phase = Phase::Failed;
    state.failed_reason = Some(reason.into());
    build_state::save(image, config, state, Some(&state.snapshot_id)).await?;
    let mut status = image.status.clone().unwrap_or_default();
    status.conditions = vec![condition(generation(image), false, reason)];
    if let Some(operation) = status.operation.as_mut() {
        operation.phase = OperationPhase::Failed;
    }
    checkpoint(config, image, status).await
}
/// The journal cannot substitute for a trusted public verified-root checkpoint.
/// A completed client may disappear after publication or a cleanup interruption.
async fn recover_missing_export(
    image: &Image,
    config: &Config,
    state: &mut State,
    release: Option<&ImageRelease>,
) -> Result<bool> {
    if state.phase != Phase::Completed
        || state.generation != generation(image)
        || state.request_digest != crate::reconcile::request_digest(image)?
        || !image
            .status
            .as_ref()
            .and_then(|s| s.build.as_ref())
            .is_some_and(|b| b.snapshot_id == state.snapshot_id)
    {
        return Ok(false);
    }
    let pods: Api<Pod> = Api::namespaced(config.client.clone(), &config.namespace);
    if pods
        .get_opt(&direct::client_name(&state.snapshot_id)?)
        .await?
        .is_some()
    {
        return Ok(false);
    }
    let operation = image.status.as_ref().and_then(|s| s.operation.as_ref());
    let trusted = operation.is_some_and(|op| {
        op.identity.image_uid == state.image_uid
            && op.identity.generation == state.generation
            && op.identity.version == state.version
            && op.identity.snapshot == state.capture.ordinary_digest
            && op.root.is_some()
            && op.root == state.root
            && matches!(
                op.phase,
                OperationPhase::Verified | OperationPhase::Publishing | OperationPhase::Published
            )
    });
    if !trusted {
        fail_completed(image, config, state, "BuildExportUnavailable").await?;
        return Ok(true);
    }
    if !refs_unchanged_or_gone(config, &state.frozen.references, false).await? {
        fail_completed(image, config, state, "InputChangedDuringBuild").await?;
        return Ok(true);
    }
    if release.is_some_and(|r| {
        r.status.as_ref().is_some_and(|s| s.verified) && Some(&r.spec.root) != state.root.as_ref()
    }) {
        fail_completed(image, config, state, "VersionConflict").await?;
        return Ok(true);
    }
    let root = state.root.as_ref().context("MissingVerifiedBuildRoot")?;
    let (registry, _tunnel) = registry(config, &state.image_uid).await?;
    let verified = match registry
        .fetch_verified(root, &image.spec.platform, &config.cancel)
        .await
    {
        Ok(verified) => verified,
        Err(_) => {
            fail_completed(image, config, state, "BuildExportUnavailable").await?;
            return Ok(true);
        }
    };
    let receipt = state
        .execution
        .clone()
        .context("MissingBuildExecutionReceipt")?;
    let provenance = BuildProvenance {
        format: "Containerfile".into(),
        adapter_version: "kimage.dev/containerfile-v1".into(),
        inputs: state
            .frozen
            .references
            .iter()
            .map(|r| kimage_core::build::ObjectIdentity {
                kind: r.kind.clone(),
                name: r.name.clone(),
                uid: r.uid.clone(),
                resource_version: r.resource_version.clone(),
            })
            .collect(),
        execution: receipt.clone(),
    };
    let mut status = image.status.clone().unwrap_or_default();
    status
        .build
        .as_mut()
        .context("MissingBuildStatus")?
        .execution = Some(receipt);
    let mut operation = operation.cloned().context("MissingBuildOperation")?;
    operation.phase = OperationPhase::Verified;
    crate::reconcile::publish_artifact(
        image,
        config,
        VerifiedArtifact {
            verified,
            snapshot: state.capture.ordinary_digest.clone(),
            provenance: Some(provenance),
        },
        operation,
        status,
    )
    .await?;
    state.phase = Phase::Published;
    state.published_fingerprint = Some(state.fingerprint.clone());
    state.published_references = state.frozen.references.clone();
    build_state::save(image, config, state, Some(&state.snapshot_id)).await?;
    build_inputs::cleanup(image, config, &state.snapshot_id).await?;
    Ok(true)
}
struct Attempt {
    image: Image,
    state: Option<State>,
    record: Option<Record>,
    id: String,
    submitted: bool,
    completed: bool,
}
async fn run(
    attempt: &mut Attempt,
    config: &Config,
    settings: &Settings,
    lease: &WorkerLease,
    previous: Option<State>,
    release: Option<&ImageRelease>,
    cancel: &CancellationToken,
) -> Result<()> {
    let approved = match direct::approved(config, settings, false).await {
        Ok(approved) => approved,
        Err(error) => {
            if let (Some(previous), Some(release)) = (&previous, release)
                && release.status.as_ref().is_some_and(|s| s.verified)
                && previous.published_fingerprint.is_some()
                && release.spec.request_digest == crate::reconcile::request_digest(&attempt.image)?
                && refs_unchanged_or_gone(config, &previous.published_references, false).await?
            {
                retained(&attempt.image, config, release).await?;
                return Ok(());
            }
            return Err(error);
        }
    };
    // A changed generation/version must retire the old RPC before a new solve.
    let old = build_state::for_cleanup(&attempt.image, config).await?;
    let old_id = attempt
        .image
        .status
        .as_ref()
        .and_then(|s| s.build.as_ref())
        .map(|b| b.snapshot_id.clone());
    if let Some(old_id) = old_id.filter(|id| id != &attempt.id) {
        direct::remove_client(config, &old_id).await?;
        if let Some(old) = old
            .as_ref()
            .filter(|s| s.snapshot_id == old_id && s.phase == Phase::Submitted)
        {
            direct::fence(config, settings, &old.worker_uid).await?;
            build_inputs::cleanup(&attempt.image, config, &old_id).await?;
            if old.worker_uid == approved.pod.uid().unwrap_or_default() {
                anyhow::bail!("BuilderUnavailable");
            }
        }
        build_inputs::cleanup(&attempt.image, config, &old_id).await?;
    }
    if let Some(previous_holder) = &lease.previous_holder {
        direct::remove_client(config, previous_holder).await?;
        direct::fence(
            config,
            settings,
            &approved.pod.uid().context("MissingWorkerUid")?,
        )
        .await?;
        anyhow::bail!("BuilderUnavailable");
    }
    let mut state = if let Some(state) = attempt.state.clone() {
        let record = direct_client::record(&attempt.image, config, &attempt.id)
            .await?
            .context("BuildSessionRecordMissing")?;
        ensure!(
            state.worker_uid == approved.pod.uid().context("MissingWorkerUid")?
                || state.phase == Phase::Completed,
            "BuilderWorkerChanged"
        );
        attempt.record = Some(record);
        state
    } else {
        let Source::Build(build) = &attempt.image.spec.source else {
            anyhow::bail!("UnsupportedBuildSource")
        };
        let frozen = build_inputs::freeze(&attempt.image, config, build, &attempt.id).await?;
        let record = direct_client::create(
            &attempt.image,
            config,
            settings,
            &approved,
            frozen,
            &attempt.id,
        )
        .await?;
        attempt.record = Some(record.clone());
        wait(config, &attempt.id, "Captured", cancel, 180).await?;
        let capture: Capture = serde_json::from_slice(
            &direct_client::exec(config, &attempt.id, vec!["build-capture-receipt".into()]).await?,
        )
        .context("InvalidCaptureReceipt")?;
        let fingerprint =
            build_state::fingerprint(&attempt.image, &capture, &record.frozen.references)?;
        if let (Some(previous), Some(release)) = (&previous, release)
            && release.status.as_ref().is_some_and(|s| s.verified)
            && previous.published_fingerprint.as_deref() == Some(&fingerprint)
        {
            direct::remove_client(config, &attempt.id).await?;
            build_inputs::cleanup(&attempt.image, config, &attempt.id).await?;
            retained(&attempt.image, config, release).await?;
            return Ok(());
        }
        if let Some(previous) = &previous
            && previous.phase == Phase::Failed
            && previous.fingerprint == fingerprint
            && previous.generation == generation(&attempt.image)
            && previous.failed_reason.as_deref().is_some_and(permanent)
        {
            direct::remove_client(config, &attempt.id).await?;
            build_inputs::cleanup(&attempt.image, config, &attempt.id).await?;
            let mut status = attempt.image.status.clone().unwrap_or_default();
            status.conditions = vec![condition(
                generation(&attempt.image),
                false,
                previous.failed_reason.as_deref().unwrap_or("BuildFailed"),
            )];
            if let Some(op) = status.operation.as_mut() {
                op.phase = OperationPhase::Failed;
            }
            checkpoint(config, &attempt.image, status).await?;
            return Ok(());
        }
        build_inputs::capture_quiescent(config, &record.frozen).await?;
        attempt.image = adopt(
            &attempt.image,
            config,
            &attempt.id,
            &capture.ordinary_digest,
            "Captured",
        )
        .await?;
        let state = State {
            image_uid: attempt.image.uid().unwrap(),
            version: attempt.image.spec.version.clone(),
            generation: generation(&attempt.image),
            snapshot_id: attempt.id.clone(),
            request_digest: crate::reconcile::request_digest(&attempt.image)?,
            fingerprint,
            frozen: record.frozen,
            capture,
            worker_uid: approved.pod.uid().context("MissingWorkerUid")?,
            phase: Phase::Captured,
            root: None,
            execution: None,
            published_fingerprint: previous
                .as_ref()
                .and_then(|s| s.published_fingerprint.clone()),
            published_references: previous
                .as_ref()
                .map(|s| s.published_references.clone())
                .unwrap_or_default(),
            failed_reason: None,
        };
        build_state::save(
            &attempt.image,
            config,
            &state,
            previous.as_ref().map(|s| s.snapshot_id.as_str()),
        )
        .await?;
        state
    };
    attempt.state = Some(state.clone());
    let monitor = monitor(
        &attempt.image,
        config,
        &attempt.id,
        state.frozen.clone(),
        state.worker_uid.clone(),
        lease.lost.clone(),
        cancel.clone(),
    );
    let result = async {
        lease.check().await?;
        if state.phase != Phase::Completed {
            let ready = direct::approved(config, settings, true).await?;
            ensure!(
                ready.pod.uid().as_deref() == Some(&state.worker_uid),
                "BuilderWorkerChanged"
            );
            ensure!(
                direct::worker_matches(config, &state.worker_uid).await?,
                "BuilderWorkerChanged"
            );
            state.phase = Phase::Submitted;
            build_state::save(&attempt.image, config, &state, Some(&attempt.id)).await?;
            attempt.state = Some(state.clone());
            attempt.submitted = true;
            let mut status = attempt.image.status.clone().unwrap_or_default();
            if let Some(op) = status.operation.as_mut()
                && op.phase == OperationPhase::Capturing
            {
                op.transition(
                    OperationPhase::Executing,
                    &state.image_uid,
                    state.generation,
                )?;
            }
            status.conditions = vec![condition(state.generation, false, "Executing")];
            checkpoint(config, &attempt.image, status).await?;
            lease.check().await?;
            ensure!(
                direct::worker_matches(config, &state.worker_uid).await?,
                "BuilderWorkerChanged"
            );
            direct_client::exec(
                config,
                &attempt.id,
                vec![
                    "build-start".into(),
                    "--snapshot-id".into(),
                    attempt.id.clone(),
                ],
            )
            .await?;
        }
        let completed = wait(config, &attempt.id, "Completed", cancel, 650).await?;
        attempt.completed = true;
        build_inputs::unchanged(config, &state.frozen).await?;
        lease.check().await?;
        let summary: crate::containerfile::SolveSummary =
            serde_json::from_value(completed["summary"].clone()).context("InvalidSolveSummary")?;
        let verified = stream(config, &attempt.id, cancel).await?;
        if let Some(root) = &state.root {
            ensure!(root == verified.root(), "OperationResultConflict");
        }
        let receipt = ExecutionReceipt {
            strategy: kimage_core::build::Strategy::Direct,
            builder: "default".into(),
            engine: "BuildKit".into(),
            engine_version: "0.33.0".into(),
            adapter_version: "kimage.dev/containerfile-v1".into(),
            worker_uid: state.worker_uid.clone(),
            engine_worker_id: summary.engine_worker_id,
            engine_operation_id: summary.engine_operation_id,
            attempt_id: attempt.id.clone(),
            platform: attempt.image.spec.platform.clone(),
            cache_mode: summary.effective_cache_mode,
            cached_execution_steps: summary.cached_run_vertices,
            executed_steps: summary.executed_run_vertices,
        };
        state.phase = Phase::Completed;
        state.root = Some(verified.root().clone());
        state.execution = Some(receipt.clone());
        build_state::save(&attempt.image, config, &state, Some(&attempt.id)).await?;
        attempt.state = Some(state.clone());
        let mut status = attempt.image.status.clone().unwrap_or_default();
        status
            .build
            .as_mut()
            .context("MissingBuildStatus")?
            .execution = Some(receipt.clone());
        let operation = status.operation.clone().context("MissingBuildOperation")?;
        let provenance = BuildProvenance {
            format: "Containerfile".into(),
            adapter_version: "kimage.dev/containerfile-v1".into(),
            inputs: state
                .frozen
                .references
                .iter()
                .map(|r| kimage_core::build::ObjectIdentity {
                    kind: r.kind.clone(),
                    name: r.name.clone(),
                    uid: r.uid.clone(),
                    resource_version: r.resource_version.clone(),
                })
                .collect(),
            execution: receipt,
        };
        crate::reconcile::cleanup_unacknowledged(config, &attempt.image).await?;
        crate::reconcile::publish_artifact(
            &attempt.image,
            config,
            VerifiedArtifact {
                verified,
                snapshot: state.capture.ordinary_digest.clone(),
                provenance: Some(provenance),
            },
            operation,
            status,
        )
        .await?;
        state.phase = Phase::Published;
        state.published_fingerprint = Some(state.fingerprint.clone());
        state.published_references = state.frozen.references.clone();
        build_state::save(&attempt.image, config, &state, Some(&attempt.id)).await?;
        attempt.state = Some(state);
        direct::remove_client(config, &attempt.id).await?;
        build_inputs::cleanup(&attempt.image, config, &attempt.id).await?;
        Ok::<(), anyhow::Error>(())
    }
    .await;
    monitor.abort();
    result
}
pub async fn entry(
    image: Arc<Image>,
    config: Arc<Config>,
) -> Result<kube::runtime::controller::Action> {
    match reconcile(image.clone(), config.clone()).await {
        Ok(action) => Ok(action),
        Err(error) => {
            let reason = reason(&error);
            crate::metrics::failure(permanent(&reason));
            if reason == "StaleOperation" {
                return Ok(kube::runtime::controller::Action::requeue(
                    Duration::from_secs(1),
                ));
            }
            let mut status = image.status.clone().unwrap_or_default();
            status.conditions = vec![condition(generation(&image), false, &reason)];
            let mut retry = condition(generation(&image), true, "Backoff");
            retry.type_ = "RetryAfter".into();
            retry.message = (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs()
                + 30)
                .to_string();
            status.conditions.push(retry);
            match checkpoint(&config, &image, status).await {
                Ok(()) => {}
                Err(error) if error.to_string() == "StaleOperation" => {}
                Err(error) => return Err(error),
            }
            tracing::warn!(image=%image.name_any(),reason,"Direct prerequisite deferred");
            Ok(kube::runtime::controller::Action::requeue(
                Duration::from_secs(30),
            ))
        }
    }
}
pub async fn reconcile(
    image: Arc<Image>,
    config: Arc<Config>,
) -> Result<kube::runtime::controller::Action> {
    use kube::runtime::controller::Action;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    if let Some(delay) = crate::reconcile::retry_remaining(&image, now) {
        return Ok(Action::requeue(Duration::from_secs(delay)));
    }
    image.spec.validate()?;
    let settings = config
        .direct
        .as_ref()
        .context("BuilderConfigurationRequired")?;
    let mut previous = build_state::load(&image, &config).await?;
    let releases: Api<ImageRelease> = Api::namespaced(config.client.clone(), &config.namespace);
    let release_name = kimage_core::catalog::release_name(
        &image.uid().context("MissingImageUid")?,
        &image.spec.version,
    );
    let release = releases.get_opt(&release_name).await?;
    if let Some(release) = &release {
        ensure!(
            release.spec.image_uid == image.uid().unwrap()
                && release.spec.platform == image.spec.platform
                && release.metadata.deletion_timestamp.is_none(),
            "VersionConflict"
        );
        if release.status.as_ref().is_some_and(|s| s.verified) {
            if let Some(state) = previous.as_mut()
                && state.root.as_ref() == Some(&release.spec.root)
                && state.request_digest == release.spec.request_digest
                && state.execution.as_ref().is_some_and(|e| {
                    release
                        .spec
                        .provenance
                        .as_ref()
                        .is_some_and(|p| p.execution.attempt_id == e.attempt_id)
                })
            {
                state.published_fingerprint = Some(state.fingerprint.clone());
                state.published_references = state.frozen.references.clone();
            }
            if let Some(state) = &previous
                && state.published_fingerprint.is_some()
                && release.spec.request_digest == crate::reconcile::request_digest(&image)?
                && refs_unchanged_or_gone(&config, &state.published_references, true).await?
            {
                retained(&image, &config, release).await?;
                return Ok(Action::requeue(Duration::from_secs(30)));
            }
        } else if recover_candidate(&image, &config, release, previous.as_mut()).await? {
            return Ok(Action::requeue(Duration::from_secs(30)));
        }
    }
    if let Some(state) = previous.as_mut() {
        if recover_missing_export(&image, &config, state, release.as_ref()).await? {
            return Ok(Action::requeue(Duration::from_secs(1)));
        }
        if state.phase == Phase::Failed
            && state.generation == generation(&image)
            && state.request_digest == crate::reconcile::request_digest(&image)?
            && state.frozen.bindings.is_empty()
            && state.failed_reason.as_deref().is_some_and(permanent)
            && refs_unchanged_or_gone(&config, &state.frozen.references, false).await?
        {
            let mut status = image.status.clone().unwrap_or_default();
            status.conditions = vec![condition(
                generation(&image),
                false,
                state.failed_reason.as_deref().unwrap_or("BuildFailed"),
            )];
            if let Some(op) = status.operation.as_mut() {
                op.phase = OperationPhase::Failed;
            }
            checkpoint(&config, &image, status).await?;
            return Ok(Action::requeue(Duration::from_secs(30)));
        }
    }
    let resumable = previous.as_ref().is_some_and(|state| {
        state.generation == generation(&image)
            && state.request_digest == crate::reconcile::request_digest(&image).unwrap_or_default()
            && matches!(
                state.phase,
                Phase::Captured | Phase::Submitted | Phase::Completed
            )
            && image
                .status
                .as_ref()
                .and_then(|s| s.build.as_ref())
                .is_some_and(|b| b.snapshot_id == state.snapshot_id)
    });
    let id = if resumable {
        previous.as_ref().unwrap().snapshot_id.clone()
    } else {
        nonce()?
    };
    let mut attempt = Attempt {
        image: (*image).clone(),
        state: if resumable { previous.clone() } else { None },
        record: None,
        id: id.clone(),
        submitted: resumable
            && previous
                .as_ref()
                .is_some_and(|s| s.phase == Phase::Submitted),
        completed: resumable
            && previous
                .as_ref()
                .is_some_and(|s| s.phase == Phase::Completed),
    };
    let lease = match WorkerLease::acquire(&config, settings, &id).await {
        Ok(lease) => lease,
        Err(error) if error.to_string() == "BuilderBusy" => {
            return Ok(Action::requeue(Duration::from_secs(3)));
        }
        Err(error) => return Err(error),
    };
    let cancel = config.cancel.child_token();
    let source_monitor = {
        let images: Api<Image> = Api::namespaced(config.client.clone(), &config.namespace);
        let expected = image.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {_=cancel.cancelled()=>break,_=tokio::time::sleep(Duration::from_secs(1))=>{}}
                let current = images.get_opt(&expected.name_any()).await;
                if !matches!(current,Ok(Some(ref current)) if current.uid()==expected.uid() &&current.metadata.generation==expected.metadata.generation &&current.metadata.deletion_timestamp.is_none())
                {
                    cancel.cancel();
                    break;
                }
            }
        })
    };
    let mut measured = crate::metrics::Observation::begin(crate::metrics::Stage::Build);
    let outcome = run(
        &mut attempt,
        &config,
        settings,
        &lease,
        previous,
        release.as_ref(),
        &cancel,
    )
    .await;
    source_monitor.abort();
    if outcome.is_ok() {
        measured.success();
    }
    drop(measured);
    let mut safe_release = true;
    if let Err(error) = outcome {
        let mut reason = reason(&error);
        if !attempt.completed
            || permanent(&reason)
            || matches!(
                reason.as_str(),
                "InputChangedDuringBuild"
                    | "StaleOperation"
                    | "OperationCancelled"
                    | "BuilderWorkerChanged"
            )
        {
            let cleanup = async {
                direct::remove_client(&config, &id).await?;
                if attempt.submitted && !attempt.completed {
                    lease.check().await?;
                    if let Some(uid) = direct::owned_worker_uid(&config, settings).await? {
                        direct::fence(&config, settings, &uid).await?;
                    }
                }
                build_inputs::cleanup(&attempt.image, &config, &id).await?;
                Ok::<(), anyhow::Error>(())
            }
            .await;
            if cleanup.is_err() {
                reason = "CancellationPending".into();
                safe_release = false;
            }
        }
        if matches!(
            reason.as_str(),
            "StaleOperation" | "InputChangedDuringBuild"
        ) {
            if let Some(state) = attempt.state.as_mut() {
                state.phase = Phase::Failed;
                state.failed_reason = Some(reason.clone());
                let _ = build_state::save(&attempt.image, &config, state, Some(&id)).await;
            }
            if safe_release {
                let _ = lease.release().await;
            }
            return Ok(Action::requeue(Duration::from_secs(1)));
        }
        let api: Api<Image> = Api::namespaced(config.client.clone(), &config.namespace);
        let current = api.get_opt(&attempt.image.name_any()).await?;
        if let Some(current) = current
            && current.uid() == image.uid()
            && current.metadata.generation == image.metadata.generation
        {
            if attempt
                .image
                .status
                .as_ref()
                .and_then(|s| s.build.as_ref())
                .map(|s| s.snapshot_id.as_str())
                != Some(&id)
            {
                attempt.image = adopt(
                    &attempt.image,
                    &config,
                    &id,
                    &oci::digest(b"capture unavailable"),
                    &reason,
                )
                .await?;
            }
            let mut status = api
                .get(&attempt.image.name_any())
                .await?
                .status
                .unwrap_or_default();
            let failed = permanent(&reason);
            if let Some(operation) = status.operation.as_mut() {
                operation.attempt = operation.attempt.saturating_add(1);
            }
            let delay = if failed {
                30
            } else {
                2_u64
                    .saturating_pow(
                        status
                            .operation
                            .as_ref()
                            .map(|op| op.attempt)
                            .unwrap_or(1)
                            .min(7),
                    )
                    .min(120)
            };
            status.conditions = vec![condition(generation(&attempt.image), false, &reason)];
            let mut deadline = condition(generation(&attempt.image), true, "Backoff");
            deadline.type_ = "RetryAfter".into();
            let failure_now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs();
            deadline.message = failure_now.saturating_add(delay).to_string();
            status.conditions.push(deadline);
            if let Some(operation) = status.operation.as_mut() {
                if failed {
                    operation.phase = OperationPhase::Failed;
                } else if !attempt.completed {
                    operation.phase = OperationPhase::Capturing;
                }
            }
            if let Some(state) = attempt.state.as_mut() {
                if safe_release && (!attempt.completed || failed) {
                    state.phase = Phase::Failed;
                    state.failed_reason = Some(reason.clone());
                }
                let _ = build_state::save(&attempt.image, &config, state, Some(&id)).await;
            }
            checkpoint(&config, &attempt.image, status).await?;
            crate::metrics::failure(failed);
        }
        tracing::warn!(image=%image.name_any(),reason,"Direct operation deferred");
    }
    if safe_release {
        lease.release().await?;
    }
    Ok(Action::requeue(Duration::from_secs(30)))
}

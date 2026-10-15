//! Independent retained-artifact exporter; no build or worker dependency.
use crate::{
    api::{Image, ImageRelease},
    export_api::{Destination, Status, Target},
    operation_lease::OperationLease,
    registry::Registry,
};
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use k8s_openapi::{api::core::v1::Secret, jiff::Timestamp};
use kube::{
    Api, Client, ResourceExt,
    api::{ListParams, Patch, PatchParams},
};
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::PathBuf, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;
#[derive(Clone)]
pub struct Config {
    pub client: Client,
    pub namespace: String,
    pub system_namespace: String,
    pub installation: String,
    pub internal_origin: reqwest::Url,
    pub internal_ca: PathBuf,
    pub internal_password: PathBuf,
    pub approved_origins: BTreeSet<String>,
    pub cancel: CancellationToken,
    pub concurrency: Arc<tokio::sync::Semaphore>,
}
struct Credentials {
    basic: Option<(String, String)>,
    ca: Option<Vec<u8>>,
    revision: String,
}
async fn credentials(config: &Config, destination: &Destination) -> Result<Credentials> {
    destination.validate()?;
    let (origin, authority, _) = destination.address()?;
    ensure!(
        config.approved_origins.contains(origin.as_str()),
        "ExportDestinationNotApproved"
    );
    let api: Api<Secret> = Api::namespaced(config.client.clone(), &config.namespace);
    let mut revision = Vec::new();
    let mut basic = None;
    if let Some(reference) = &destination.credentials_secret_ref {
        let secret = api
            .get(&reference.name)
            .await
            .context("ExportCredentialUnavailable")?;
        ensure!(
            secret.type_.as_deref() == Some("kubernetes.io/dockerconfigjson"),
            "InvalidExportCredentials"
        );
        let data = secret
            .data
            .as_ref()
            .and_then(|d| d.get(".dockerconfigjson"))
            .context("InvalidExportCredentials")?;
        ensure!(data.0.len() <= 65536, "InvalidExportCredentials");
        let value: Value = serde_json::from_slice(&data.0).context("InvalidExportCredentials")?;
        let entry = &value["auths"][&authority];
        ensure!(entry.is_object(), "ExportCredentialAuthorityMismatch");
        let (username, password) = if let Some(auth) = entry["auth"].as_str() {
            let decoded = STANDARD.decode(auth).context("InvalidExportCredentials")?;
            ensure!(decoded.len() <= 8192, "InvalidExportCredentials");
            let decoded = String::from_utf8(decoded).context("InvalidExportCredentials")?;
            let (user, password) = decoded
                .split_once(':')
                .context("InvalidExportCredentials")?;
            (user.to_owned(), password.to_owned())
        } else {
            (
                entry["username"]
                    .as_str()
                    .context("InvalidExportCredentials")?
                    .into(),
                entry["password"]
                    .as_str()
                    .context("InvalidExportCredentials")?
                    .into(),
            )
        };
        ensure!(
            !username.is_empty()
                && username.len() <= 1024
                && !username.contains([':', '\r', '\n'])
                && !password.is_empty()
                && password.len() <= 4096,
            "InvalidExportCredentials"
        );
        basic = Some((username, password));
        revision.push((secret.uid(), secret.resource_version()));
    }
    let ca = if let Some(reference) = &destination.ca_secret_ref {
        let secret = api
            .get(&reference.name)
            .await
            .context("ExportCaUnavailable")?;
        let data = secret
            .data
            .as_ref()
            .and_then(|d| d.get("ca.crt"))
            .context("InvalidExportCa")?;
        ensure!(
            !data.0.is_empty() && data.0.len() <= 65536,
            "InvalidExportCa"
        );
        reqwest::Certificate::from_pem(&data.0).context("InvalidExportCa")?;
        revision.push((secret.uid(), secret.resource_version()));
        Some(data.0.clone())
    } else {
        None
    };
    Ok(Credentials {
        basic,
        ca,
        revision: kimage_core::oci::digest(&serde_json::to_vec(&revision)?),
    })
}
async fn selected(config: &Config, image: &Image) -> Result<Option<ImageRelease>> {
    let api: Api<ImageRelease> = Api::namespaced(config.client.clone(), &config.namespace);
    let mut token = None;
    let mut count = 0;
    let mut found = None;
    loop {
        let mut params = ListParams::default().limit(256);
        params.continue_token = token;
        let page = api.list(&params).await?;
        count += page.items.len();
        ensure!(count <= 4096, "ExportReleaseInventoryTooLarge");
        for release in page.items {
            if Some(release.spec.image_uid.clone()) == image.uid()
                && release.spec.image_name == image.name_any()
                && release.spec.version == image.spec.version
                && release.metadata.deletion_timestamp.is_none()
                && release.status.as_ref().is_some_and(|s| {
                    s.verified && s.observed_generation == release.metadata.generation.unwrap_or(1)
                })
            {
                ensure!(found.is_none(), "AmbiguousExportRelease");
                found = Some(release);
            }
        }
        token = page.metadata.continue_.filter(|v| !v.is_empty());
        if token.is_none() {
            return Ok(found);
        }
    }
}
async fn unchanged(config: &Config, image: &Image, target: &Target, revision: &str) -> Result<()> {
    ensure!(!config.cancel.is_cancelled(), "ExportCancelled");
    let current: Image = Api::namespaced(config.client.clone(), &config.namespace)
        .get(&image.name_any())
        .await
        .context("ExportCancelled")?;
    ensure!(
        current.uid() == image.uid()
            && current.metadata.deletion_timestamp.is_none()
            && current.spec.version == image.spec.version
            && current.spec.exports.iter().find(|t| t.name == target.name) == Some(target),
        "ExportCancelled"
    );
    ensure!(
        credentials(config, &target.registry).await?.revision == revision,
        "ExportCredentialsChanged"
    );
    Ok(())
}
async fn transfer(
    config: &Config,
    image: &Image,
    target: &Target,
    release: &ImageRelease,
    private: Credentials,
) -> Result<()> {
    let (_, authority, repository) = target.registry.address()?;
    // Sorted acquisition serializes all of kimage's writers for each tag across
    // Images and exporter replicas. It cannot serialize unrelated remote writers.
    let mut leases = Vec::new();
    let result = async {
        let mut tags = target.registry.tags.clone();
        tags.sort();
        for tag in &tags {
            let key =
                kimage_core::oci::digest(format!("{authority}/{repository}:{tag}").as_bytes());
            let name = format!("{}-export-{}", config.installation, &key[7..27]);
            leases.push(
                OperationLease::acquire(
                    config.client.clone(),
                    &config.system_namespace,
                    &config.installation,
                    &name,
                    &key[7..],
                )
                .await?,
            );
        }
        let (origin, _, repository) = target.registry.address()?;
        let remote = Registry::external(origin, repository, private.ca.as_deref(), private.basic)?;
        for tag in &tags {
            if let Some(digest) = remote.tag_digest(tag).await? {
                ensure!(digest == release.spec.root.digest, "TagConflict");
            }
        }
        let local = Registry::new(
            config.internal_origin.clone(),
            format!("{}/{}", config.namespace, release.spec.image_uid),
            &std::fs::read(&config.internal_ca)?,
            None,
            "reader".into(),
            std::fs::read_to_string(&config.internal_password)?
                .trim_end()
                .into(),
        )?;
        let cancel = CancellationToken::new();
        let retained = local
            .fetch_verified(&release.spec.root, &release.spec.platform, &cancel)
            .await?;
        unchanged(config, image, target, &private.revision).await?;
        for lease in &leases {
            lease.check().await?;
        }
        remote.publish(&retained, &cancel).await?;
        // Independent full graph readback binds remote success to original bytes.
        remote
            .fetch_verified(&release.spec.root, &release.spec.platform, &cancel)
            .await?;
        for tag in &tags {
            unchanged(config, image, target, &private.revision).await?;
            for lease in &leases {
                lease.check().await?;
            }
            remote.tag_never(&retained, tag).await?;
        }
        Ok::<_, anyhow::Error>(())
    }
    .await;
    for lease in leases {
        let released = lease.release().await;
        if result.is_ok() {
            released?;
        }
    }
    result
}
fn reason(error: &anyhow::Error) -> String {
    const KNOWN: &[&str] = &[
        "TagConflict",
        "ExportCancelled",
        "ExportCredentialsChanged",
        "ExportDestinationNotApproved",
        "ExportCredentialAuthorityMismatch",
        "ExportCredentialUnavailable",
        "InvalidExportCredentials",
        "ExportCaUnavailable",
        "InvalidExportCa",
        "ExportAuthenticationFailed",
        "UnsupportedRemoteMediaType",
        "RemoteTagChanged",
        "RemoteTagDigestMismatch",
        "OperationBusy",
        "OperationLeaseLost",
    ];
    error
        .chain()
        .find_map(|e| {
            KNOWN
                .iter()
                .find(|r| e.to_string() == **r)
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "ExportTransportOrVerificationFailed".into())
}
async fn monitored(
    config: &Config,
    image: &Image,
    target: &Target,
    release: &ImageRelease,
    private: Credentials,
) -> Result<()> {
    let revision = private.revision.clone();
    let monitor = async {
        loop {
            tokio::select! {_=config.cancel.cancelled()=>anyhow::bail!("ExportCancelled"),_=tokio::time::sleep(Duration::from_secs(2))=>{}}
            tokio::time::timeout(
                Duration::from_secs(10),
                unchanged(config, image, target, &revision),
            )
            .await
            .context("ExportCancelled")??;
        }
        #[allow(unreachable_code)]
        Ok::<(), anyhow::Error>(())
    };
    tokio::select! {
        value=tokio::time::timeout(Duration::from_secs(600),transfer(config,image,target,release,private))=>value.context("ExportDeadlineExceeded")?,
        value=monitor=>value,
    }
}
async fn acknowledge(config: &Config, image: &Image, statuses: Vec<Status>) -> Result<()> {
    let api: Api<Image> = Api::namespaced(config.client.clone(), &config.namespace);
    let current = api.get(&image.name_any()).await?;
    ensure!(
        current.uid() == image.uid()
            && current.metadata.generation == image.metadata.generation
            && current.metadata.deletion_timestamp.is_none(),
        "ExportCancelled"
    );
    if current.status.as_ref().map(|s| &s.exports) == Some(&statuses) {
        return Ok(());
    }
    let mut patch = json!({"metadata":{"uid":current.uid(),"resourceVersion":current.resource_version()},"status":{"exports":statuses}});
    // ImageStatus requires observedGeneration. An independent PendingRelease
    // acknowledgment may be the first writer while publication is stopped.
    // Zero explicitly means publication has not acknowledged any generation.
    if current.status.is_none() {
        patch["status"]["observedGeneration"] = json!(0);
    }
    api.patch_status(
        &image.name_any(),
        &PatchParams::default(),
        &Patch::Merge(patch),
    )
    .await?;
    Ok(())
}
pub async fn reconcile(
    image: Arc<Image>,
    config: Arc<Config>,
) -> Result<kube::runtime::controller::Action> {
    use kube::runtime::controller::Action;
    if image.metadata.deletion_timestamp.is_some() {
        return Ok(Action::await_change());
    }
    crate::export_api::validate(&image.spec.exports)?;
    if image.spec.exports.is_empty() {
        if image.status.as_ref().is_some_and(|s| !s.exports.is_empty()) {
            acknowledge(&config, &image, Vec::new()).await?;
        }
        return Ok(Action::await_change());
    }
    let _permit = config.concurrency.acquire().await?;
    let release = selected(&config, &image).await?;
    let mut statuses = Vec::new();
    for target in &image.spec.exports {
        let now = Timestamp::now();
        let private = credentials(&config, &target.registry).await;
        let revision = private
            .as_ref()
            .map(|v| v.revision.as_str())
            .unwrap_or("unavailable");
        let fingerprint = kimage_core::oci::digest(&serde_json::to_vec(
            &json!({"target":target,"credentialRevision":revision,"releaseUid":release.as_ref().and_then(ResourceExt::uid),"root":release.as_ref().map(|r|&r.spec.root)}),
        )?);
        let old = image.status.as_ref().and_then(|s| {
            s.exports
                .iter()
                .find(|s| s.name == target.name && s.configuration_digest == fingerprint)
        });
        if let Some(old) = old
            && (old.phase == "Complete" || old.retry_after > now.as_second())
        {
            let mut current = old.clone();
            current.configuration_generation = image.metadata.generation.unwrap_or(1);
            statuses.push(current);
            continue;
        }
        let attempts = old.map_or(1, |s| s.attempts.saturating_add(1).min(32));
        let mut status = Status {
            name: target.name.clone(),
            configuration_generation: image.metadata.generation.unwrap_or(1),
            configuration_digest: fingerprint,
            release_uid: release.as_ref().and_then(ResourceExt::uid),
            local_digest: release.as_ref().map(|r| r.spec.root.digest.clone()),
            remote_digest: None,
            remote_reference: release
                .as_ref()
                .map(|r| format!("{}@{}", target.registry.repository, r.spec.root.digest)),
            phase: "PendingRelease".into(),
            attempt_time: now.to_string(),
            attempts,
            retry_after: now.as_second() + 10,
            reason: None,
        };
        if let Some(release) = &release {
            // Persist the selected immutable identity before any external I/O.
            let mut pending = status.clone();
            pending.phase = "Transferring".into();
            let mut checkpoint = statuses.clone();
            checkpoint.push(pending);
            acknowledge(&config, &image, checkpoint).await?;
            let result = match private {
                Ok(private) => monitored(&config, &image, target, release, private).await,
                Err(error) => Err(error),
            };
            match result {
                Ok(()) => {
                    status.phase = "Complete".into();
                    status.remote_digest = Some(release.spec.root.digest.clone());
                    status.retry_after = 0;
                }
                Err(error) => {
                    let why = reason(&error);
                    if why == "ExportCancelled" {
                        return Ok(Action::requeue(Duration::from_secs(2)));
                    }
                    status.phase = if why == "TagConflict" {
                        "TagConflict".into()
                    } else {
                        "Retrying".into()
                    };
                    status.retry_after = Timestamp::now().as_second()
                        + if why == "TagConflict" {
                            300
                        } else {
                            i64::from(2_u32.saturating_pow(attempts.min(8)).min(300))
                        };
                    status.reason = Some(why);
                }
            }
        }
        statuses.push(status);
    }
    acknowledge(&config, &image, statuses).await?;
    Ok(Action::requeue(Duration::from_secs(5)))
}

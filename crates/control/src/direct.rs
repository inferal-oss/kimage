//! Approved single-worker placement, exclusive lease and explicit Pod fencing.
use crate::{install::LABEL, reconcile::Config};
use anyhow::{Context, Result, ensure};
use k8s_openapi::{
    ByteString,
    api::{
        coordination::v1::{Lease, LeaseSpec},
        core::v1::{ConfigMap, Pod, Secret},
    },
    apimachinery::pkg::apis::meta::v1::{MicroTime, ObjectMeta},
    jiff::Timestamp,
};
use kube::{
    Api, ResourceExt,
    api::{DeleteParams, PostParams, Preconditions},
};
use serde_json::Value;
use std::{collections::BTreeMap, time::Duration};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct Settings {
    pub architecture: Option<String>,
    pub system_namespace: String,
    pub installation: String,
    pub builder_image: String,
}
pub struct Approved {
    pub pod: Pod,
    pub endpoint: String,
    pub tls_server_name: String,
    pub credentials: BTreeMap<String, ByteString>,
}
impl Settings {
    pub fn name(&self, suffix: &str) -> String {
        match &self.architecture {
            Some(arch) => format!("{}-{suffix}-{arch}", self.installation),
            None => format!("{}-{suffix}", self.installation),
        }
    }
}
pub async fn native_context(image: &crate::api::Image, config: &Config) -> Result<Config> {
    let mut config = config.clone();
    let Some(settings) = config.direct.as_mut() else {
        return Ok(config);
    };
    let maps: Api<ConfigMap> = Api::namespaced(config.client.clone(), &settings.system_namespace);
    let arch = &image.spec.platform.architecture;
    let Some(map) = maps
        .get_opt(&format!("{}-builders-{arch}", settings.installation))
        .await?
    else {
        if arch != "amd64" {
            settings.architecture = Some(arch.clone());
        }
        return Ok(config);
    };
    ensure!(
        map.labels().get(LABEL) == Some(&settings.installation) && map.immutable == Some(true),
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
            && builder["platforms"] == serde_json::json!([image.spec.platform.name()]),
        "UnsupportedBuilderPlatform"
    );
    let input = builder["inputImage"]
        .as_str()
        .context("MissingNativeInputImage")?;
    let worker = builder["image"]
        .as_str()
        .context("MissingNativeBuilderImage")?;
    for (reference, role) in [(input, "input"), (worker, "builder")] {
        let prefix = format!("{}/system-{role}@", settings.system_namespace);
        ensure!(
            reference
                .split_once('/')
                .is_some_and(|(authority, path)| authority
                    .parse::<std::net::SocketAddr>()
                    .is_ok_and(|a| a.ip() == std::net::Ipv4Addr::LOCALHOST && a.port() >= 1024)
                    && path
                        .strip_prefix(&prefix)
                        .is_some_and(kimage_core::oci::valid_digest)),
            "InvalidNativeComponentReference"
        );
    }
    config.node = builder["node"]
        .as_str()
        .context("MissingNativeWorkerNode")?
        .into();
    config.input_image = input.into();
    settings.architecture = Some(arch.clone());
    settings.builder_image = worker.into();
    Ok(config)
}
pub async fn approved(
    config: &Config,
    settings: &Settings,
    require_ready: bool,
) -> Result<Approved> {
    ensure!(
        kimage_core::selector::dns_label(&settings.system_namespace)
            && kimage_core::selector::dns_label(&settings.installation),
        "InvalidBuilderScope"
    );
    let maps: Api<ConfigMap> = Api::namespaced(config.client.clone(), &settings.system_namespace);
    let object = maps.get(&settings.name("builders")).await?;
    ensure!(
        object.labels().get(LABEL) == Some(&settings.installation),
        "BuilderCatalogOwnershipConflict"
    );
    let catalog: Value = serde_json::from_str(
        object
            .data
            .as_ref()
            .and_then(|d| d.get("catalog.json"))
            .context("MissingBuilderCatalog")?,
    )?;
    let builder = &catalog["builders"]["default"];
    let dns = format!(
        "{}.{}.svc",
        settings.name("builder"),
        settings.system_namespace
    );
    let tls_name = format!(
        "{}-builder.{}.svc",
        settings.installation, settings.system_namespace
    );
    ensure!(
        catalog["schema"] == 1
            && builder["engine"] == "BuildKit"
            && builder["version"] == "0.33.0"
            && builder["frontendVersion"] == "1.27.0"
            && builder["node"] == config.node
            && (builder["trustScope"] == config.namespace || builder["trustScope"] == "*")
            && builder["offline"] == true
            && builder["maxParallelism"] == 1
            && builder["endpoint"] == format!("tcp://{dns}:1234")
            && builder["tlsServerName"] == tls_name
            && builder["clientSecret"] == settings.name("builder-client"),
        "UnsupportedBuilderCatalog"
    );
    let configuration = maps.get(&settings.name("builder-config")).await?;
    ensure!(
        configuration.labels().get(LABEL) == Some(&settings.installation)
            && configuration
                .data
                .as_ref()
                .and_then(|d| d.get("buildkitd.toml"))
                .is_some_and(|s| s
                    == &crate::builder_install::configuration(
                        settings.architecture.as_deref().unwrap_or("amd64")
                    )),
        "BuilderIsolationConfigurationMismatch"
    );
    let pods: Api<Pod> = Api::namespaced(config.client.clone(), &settings.system_namespace);
    let pod = if let Some(pod) = pods.get_opt(&settings.name("builder")).await? {
        pod
    } else {
        let template = maps.get(&settings.name("builder-template")).await?;
        ensure!(
            template.labels().get(LABEL) == Some(&settings.installation)
                && template.immutable == Some(true),
            "BuilderTemplateOwnershipConflict"
        );
        let pod: Pod = serde_json::from_str(
            template
                .data
                .as_ref()
                .and_then(|d| d.get("pod.json"))
                .context("MissingBuilderTemplate")?,
        )?;
        ensure!(
            pod.name_any() == settings.name("builder")
                && pod.labels().get(LABEL) == Some(&settings.installation)
                && pod
                    .spec
                    .as_ref()
                    .is_some_and(|s| s.host_users == Some(false)
                        && s.host_network != Some(true)
                        && s.containers.len() == 1
                        && s.containers[0].image.as_deref() == Some(&settings.builder_image)
                        && !s.volumes.iter().flatten().any(|v| v.host_path.is_some())),
            "BuilderTemplateDefinitionConflict"
        );
        pods.create(&PostParams::default(), &pod).await?;
        anyhow::bail!("BuilderUnavailable");
    };
    ensure!(
        pod.labels().get(LABEL) == Some(&settings.installation),
        "BuilderPodOwnershipConflict"
    );
    let spec = pod.spec.as_ref().context("MissingBuilderPodSpec")?;
    ensure!(
        spec.containers.len() == 1
            && spec.containers[0].image.as_deref() == Some(&settings.builder_image)
            && spec.host_users == Some(false)
            && spec.host_network != Some(true)
            && spec.automount_service_account_token == Some(false)
            && !spec.volumes.iter().flatten().any(|v| v.host_path.is_some()),
        "BuilderIsolationMismatch"
    );
    if require_ready {
        ensure!(
            pod.status
                .as_ref()
                .and_then(|s| s.conditions.as_ref())
                .is_some_and(|cs| cs.iter().any(|c| c.type_ == "Ready" && c.status == "True")),
            "BuilderUnavailable"
        );
        ensure!(
            pod.metadata.deletion_timestamp.is_none(),
            "BuilderUnavailable"
        );
        let slices: Api<k8s_openapi::api::discovery::v1::EndpointSlice> =
            Api::namespaced(config.client.clone(), &settings.system_namespace);
        let endpoints = slices
            .list(&kube::api::ListParams::default().labels(&format!(
                "kubernetes.io/service-name={}",
                settings.name("builder")
            )))
            .await?;
        let ready: Vec<_> = endpoints
            .items
            .iter()
            .flat_map(|s| s.endpoints.iter().flatten())
            .filter(|e| e.conditions.as_ref().is_some_and(|c| c.ready == Some(true)))
            .collect();
        ensure!(
            !ready.is_empty()
                && ready
                    .iter()
                    .all(|e| e.target_ref.as_ref().is_some_and(|r| r.uid == pod.uid())),
            "BuilderEndpointPending"
        );
    }
    let secrets: Api<Secret> = Api::namespaced(config.client.clone(), &settings.system_namespace);
    let secret = secrets.get(&settings.name("builder-client")).await?;
    ensure!(
        secret.labels().get(LABEL) == Some(&settings.installation),
        "BuilderCredentialOwnershipConflict"
    );
    let mut credentials = BTreeMap::new();
    for key in ["ca.crt", "client.crt", "client.key"] {
        let value = secret
            .data
            .as_ref()
            .and_then(|d| d.get(key))
            .context("MissingBuilderCredential")?;
        ensure!(
            !value.0.is_empty() && value.0.len() <= 32768,
            "InvalidBuilderCredentialSize"
        );
        credentials.insert(key.into(), value.clone());
    }
    Ok(Approved {
        pod,
        endpoint: format!("tcp://{dns}:1234"),
        tls_server_name: tls_name,
        credentials,
    })
}
pub struct WorkerLease {
    pub holder: String,
    pub previous_holder: Option<String>,
    api: Api<Lease>,
    name: String,
    stop: CancellationToken,
    pub lost: CancellationToken,
}
impl WorkerLease {
    pub async fn acquire(config: &Config, settings: &Settings, holder: &str) -> Result<Self> {
        ensure!(
            holder.len() == 64 && holder.bytes().all(|c| c.is_ascii_hexdigit()),
            "InvalidBuildSnapshotId"
        );
        let api: Api<Lease> = Api::namespaced(config.client.clone(), &settings.system_namespace);
        let name = settings.name("direct-worker");
        let existing = api.get_opt(&name).await?;
        let now = Timestamp::now();
        let mut previous_holder = None;
        if let Some(current) = &existing {
            ensure!(
                current.labels().get(LABEL) == Some(&settings.installation),
                "BuilderLeaseOwnershipConflict"
            );
            if let Some(spec) = &current.spec {
                let expiry = spec.renew_time.as_ref().map(|t| {
                    t.0.as_second() + i64::from(spec.lease_duration_seconds.unwrap_or(30))
                });
                if let Some(previous) = &spec.holder_identity {
                    ensure!(expiry.is_some_and(|t| t <= now.as_second()), "BuilderBusy");
                    let previous_id = previous.split(':').next().unwrap_or("");
                    ensure!(
                        previous_id.len() == 64
                            && previous_id.bytes().all(|c| c.is_ascii_hexdigit()),
                        "InvalidPreviousBuilderLease"
                    );
                    previous_holder = Some(previous_id.to_owned());
                }
            }
        }
        let mut instance = [0_u8; 16];
        getrandom::fill(&mut instance)
            .map_err(|_| anyhow::anyhow!("OperationIdentityUnavailable"))?;
        let holder = format!("{holder}:{}", hex::encode(instance));
        let mut lease = existing.clone().unwrap_or_else(|| Lease {
            metadata: ObjectMeta {
                name: Some(name.clone()),
                labels: Some(BTreeMap::from([(
                    LABEL.into(),
                    settings.installation.clone(),
                )])),
                ..Default::default()
            },
            spec: None,
        });
        lease.spec = Some(LeaseSpec {
            holder_identity: Some(holder.clone()),
            renew_time: Some(MicroTime(now)),
            lease_duration_seconds: Some(30),
            ..Default::default()
        });
        if existing.is_some() {
            api.replace(&name, &PostParams::default(), &lease).await?;
        } else {
            api.create(&PostParams::default(), &lease).await?;
        }
        let result = Self {
            holder,
            previous_holder,
            api,
            name,
            stop: CancellationToken::new(),
            lost: CancellationToken::new(),
        };
        let stop = result.stop.clone();
        let lost = result.lost.clone();
        let api = result.api.clone();
        let name = result.name.clone();
        let holder = result.holder.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {_=stop.cancelled()=>break,_=tokio::time::sleep(Duration::from_secs(5))=>{}}
                if renew(&api, &name, &holder).await.is_err() {
                    lost.cancel();
                    break;
                }
            }
        });
        Ok(result)
    }
    pub async fn check(&self) -> Result<()> {
        ensure!(!self.lost.is_cancelled(), "BuilderLeaseLost");
        renew(&self.api, &self.name, &self.holder).await
    }
    pub async fn release(&self) -> Result<()> {
        self.stop.cancel();
        for attempt in 0..3 {
            let mut current = self.api.get(&self.name).await?;
            if current
                .spec
                .as_ref()
                .and_then(|s| s.holder_identity.as_deref())
                != Some(&self.holder)
            {
                return Ok(());
            }
            let spec = current.spec.as_mut().context("MissingBuilderLeaseSpec")?;
            spec.holder_identity = None;
            spec.renew_time = None;
            match self
                .api
                .replace(&self.name, &PostParams::default(), &current)
                .await
            {
                Ok(_) => return Ok(()),
                Err(kube::Error::Api(error)) if error.code == 409 && attempt < 2 => continue,
                Err(error) => return Err(error.into()),
            }
        }
        anyhow::bail!("BuilderLeaseReleaseConflict")
    }
}
impl Drop for WorkerLease {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}
async fn renew(api: &Api<Lease>, name: &str, holder: &str) -> Result<()> {
    let started = tokio::time::Instant::now();
    for attempt in 0..3 {
        let mut current = api.get(name).await?;
        ensure!(
            current
                .spec
                .as_ref()
                .and_then(|s| s.holder_identity.as_deref())
                == Some(holder),
            "BuilderLeaseLost"
        );
        current
            .spec
            .as_mut()
            .context("MissingBuilderLeaseSpec")?
            .renew_time = Some(MicroTime(Timestamp::now()));
        match api.replace(name, &PostParams::default(), &current).await {
            Ok(_) => {
                ensure!(
                    started.elapsed() < Duration::from_secs(10),
                    "BuilderLeaseRenewExpired"
                );
                return Ok(());
            }
            Err(kube::Error::Api(error)) if error.code == 409 && attempt < 2 => continue,
            Err(error) => return Err(error.into()),
        }
    }
    anyhow::bail!("BuilderLeaseRenewConflict")
}
pub fn client_name(id: &str) -> Result<String> {
    ensure!(
        id.len() == 64 && id.bytes().all(|c| c.is_ascii_hexdigit()),
        "InvalidBuildSnapshotId"
    );
    Ok(format!("build-{}-client", &id[..20]))
}
pub async fn remove_client(config: &Config, id: &str) -> Result<()> {
    let pods: Api<Pod> = Api::namespaced(config.client.clone(), &config.namespace);
    let name = client_name(id)?;
    for attempt in 0..3 {
        let Some(pod) = pods.get_opt(&name).await? else {
            return Ok(());
        };
        ensure!(
            pod.labels()
                .get(crate::build_inputs::SNAPSHOT_LABEL)
                .is_some_and(|v| v == &id[..40])
                && pod.owner_references().iter().any(|r| r.kind == "Image"
                    && Some(&r.uid) == pod.labels().get("kimage.dev/image-uid")),
            "BuildClientOwnershipConflict"
        );
        if pod.metadata.deletion_timestamp.is_none() {
            match pods
                .delete(
                    &name,
                    &DeleteParams {
                        grace_period_seconds: Some(1),
                        preconditions: Some(Preconditions {
                            uid: pod.uid(),
                            resource_version: pod.resource_version(),
                        }),
                        ..Default::default()
                    },
                )
                .await
            {
                Ok(_) => {}
                Err(kube::Error::Api(error)) if error.code == 404 => return Ok(()),
                Err(kube::Error::Api(error)) if error.code == 409 && attempt < 2 => continue,
                Err(error) => return Err(error.into()),
            }
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
        while pods.get_opt(&name).await?.is_some() {
            ensure!(
                tokio::time::Instant::now() < deadline,
                "BuildClientTerminationTimeout"
            );
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        return Ok(());
    }
    Ok(())
}
/// Fence a submitted or orphaned attempt, AFTER its client has been terminated.
/// Delete only the expected worker UID; a later replacement must never be killed.
pub async fn fence(config: &Config, settings: &Settings, expected_uid: &str) -> Result<()> {
    let pods: Api<Pod> = Api::namespaced(config.client.clone(), &settings.system_namespace);
    let name = settings.name("builder");
    let Some(current) = pods.get_opt(&name).await? else {
        return Ok(());
    };
    if current.uid().as_deref() != Some(expected_uid) {
        return Ok(());
    }
    ensure!(
        current.labels().get(LABEL) == Some(&settings.installation)
            && current
                .spec
                .as_ref()
                .is_some_and(|s| s.containers.len() == 1
                    && s.containers[0].image.as_deref() == Some(&settings.builder_image)),
        "BuilderFenceOwnershipConflict"
    );
    let mut replacement = current.clone();
    replacement.metadata = ObjectMeta {
        name: Some(name.clone()),
        labels: current.metadata.labels.clone(),
        ..Default::default()
    };
    replacement.status = None;
    pods.delete(
        &name,
        &DeleteParams {
            grace_period_seconds: Some(1),
            preconditions: Some(Preconditions {
                uid: current.uid(),
                resource_version: current.resource_version(),
            }),
            ..Default::default()
        },
    )
    .await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    while pods.get_opt(&name).await?.is_some() {
        ensure!(
            tokio::time::Instant::now() < deadline,
            "BuilderFenceTimeout"
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    pods.create(&PostParams::default(), &replacement).await?;
    Ok(())
}

pub async fn worker_matches(config: &Config, uid: &str) -> Result<bool> {
    let settings = config
        .direct
        .as_ref()
        .context("BuilderConfigurationRequired")?;
    let pods: Api<Pod> = Api::namespaced(config.client.clone(), &settings.system_namespace);
    Ok(pods
        .get_opt(&settings.name("builder"))
        .await?
        .is_some_and(|p| {
            p.uid().as_deref() == Some(uid)
                && p.metadata.deletion_timestamp.is_none()
                && p.labels().get(LABEL) == Some(&settings.installation)
        }))
}

pub async fn owned_worker_uid(config: &Config, settings: &Settings) -> Result<Option<String>> {
    let pods: Api<Pod> = Api::namespaced(config.client.clone(), &settings.system_namespace);
    let Some(pod) = pods.get_opt(&settings.name("builder")).await? else {
        return Ok(None);
    };
    ensure!(
        pod.labels().get(LABEL) == Some(&settings.installation)
            && pod.spec.as_ref().is_some_and(|s| s.containers.len() == 1
                && s.containers[0].image.as_deref() == Some(&settings.builder_image)),
        "BuilderFenceOwnershipConflict"
    );
    Ok(pod.uid())
}

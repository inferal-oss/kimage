//! Native variants reuse the durable single-platform build journal. Only complete
//! verified graphs from matching source snapshots may form a shared OCI index.
use crate::{
    api::{Image, ImageRelease, ImageReleaseSpec, ReleaseStatus},
    configuration,
    reconcile::{Config, checkpoint, condition},
    registry::Registry,
};
use anyhow::{Context, Result, ensure};
use k8s_openapi::api::core::v1::Node;
use kimage_core::{
    catalog::platform_release_name,
    oci::{self, Platform},
};
use kube::runtime::controller::Action;
use kube::{
    Api, ResourceExt,
    api::{ListParams, Patch, PatchParams, PostParams},
};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};

fn child_name(image: &Image, platform: &Platform) -> Result<String> {
    Ok(format!(
        "variant-{}",
        &oci::digest(&serde_json::to_vec(&(
            image.uid().context("MissingImageUid")?,
            &image.spec.version,
            platform
        ))?)[7..47]
    ))
}
fn child(image: &Image, platform: &Platform) -> Result<Image> {
    let mut spec = image.spec.clone();
    spec.platform = platform.clone();
    spec.platforms.clear();
    spec.exports.clear();
    spec.channels.clear();
    let mut child = Image::new(&child_name(image, platform)?, spec);
    child.metadata.labels = Some(BTreeMap::from([(
        "kimage.dev/platform-parent".into(),
        image.uid().context("MissingImageUid")?,
    )]));
    child.metadata.owner_references = Some(vec![
        k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference {
            api_version: "kimage.dev/v1alpha1".into(),
            kind: "Image".into(),
            name: image.name_any(),
            uid: image.uid().context("MissingImageUid")?,
            controller: Some(true),
            block_owner_deletion: Some(false),
        },
    ]);
    Ok(child)
}
fn validate_child(image: &Image, expected: &Image, actual: &Image) -> Result<()> {
    ensure!(
        actual.metadata.deletion_timestamp.is_none()
            && actual.owner_references() == expected.owner_references()
            && actual.labels().get("kimage.dev/platform-parent") == image.uid().as_ref()
            && serde_json::to_value(&actual.spec)? == serde_json::to_value(&expected.spec)?,
        "PlatformVariantConflict: use a new version for changed source"
    );
    Ok(())
}
async fn available(config: &Config) -> Result<BTreeSet<Platform>> {
    let nodes: Api<Node> = Api::all(config.client.clone());
    let list = nodes.list(&ListParams::default().limit(1024)).await?;
    ensure!(
        list.metadata
            .continue_
            .as_ref()
            .is_none_or(String::is_empty),
        "TooManyClusterNodes"
    );
    Ok(list
        .into_iter()
        .filter(|node| {
            node.metadata.deletion_timestamp.is_none()
                && node
                    .spec
                    .as_ref()
                    .is_none_or(|s| s.unschedulable != Some(true))
                && node
                    .status
                    .as_ref()
                    .and_then(|s| s.conditions.as_ref())
                    .is_some_and(|cs| cs.iter().any(|c| c.type_ == "Ready" && c.status == "True"))
        })
        .filter_map(|node| {
            let labels = node.labels();
            let platform = Platform {
                os: labels.get("kubernetes.io/os")?.clone(),
                architecture: labels.get("kubernetes.io/arch")?.clone(),
            };
            platform.supported().then_some(platform)
        })
        .collect())
}
fn registry(config: &Config, uid: &str) -> Result<Registry> {
    ensure!(
        config.registry_pod.is_none(),
        "PlatformBuildRequiresRegistryService"
    );
    Registry::new(
        config.registry_origin.clone(),
        format!("{}/{uid}", config.namespace),
        &std::fs::read(&config.registry_ca)?,
        None,
        "publisher".into(),
        std::fs::read_to_string(&config.publisher_password)?
            .trim_end()
            .into(),
    )
}
async fn current(image: &Image, config: &Config) -> Result<()> {
    let api: Api<Image> = Api::namespaced(config.client.clone(), &config.namespace);
    let now = api.get(&image.name_any()).await?;
    ensure!(
        now.uid() == image.uid()
            && now.metadata.generation == image.metadata.generation
            && now.metadata.deletion_timestamp.is_none(),
        "StaleOperation"
    );
    Ok(())
}
async fn publish(
    image: &Image,
    config: &Config,
    artifact: &oci::VerifiedImage,
    snapshot: String,
    provenance: Option<crate::api::BuildProvenance>,
) -> Result<String> {
    let uid = image.uid().context("MissingImageUid")?;
    let platform = if artifact.platforms().len() == 1 {
        artifact.platforms().first().unwrap().clone()
    } else {
        Platform::automatic()
    };
    let name = platform_release_name(&uid, &image.spec.version, &platform);
    let api: Api<ImageRelease> = Api::namespaced(config.client.clone(), &config.namespace);
    let candidate = ImageReleaseSpec {
        image_name: image.name_any(),
        image_uid: uid.clone(),
        version: image.spec.version.clone(),
        snapshot,
        root: artifact.root().clone(),
        platform,
        request_digest: crate::reconcile::request_digest(image)?,
        source_generation: image.metadata.generation.unwrap_or(1),
        publisher_version: env!("CARGO_PKG_VERSION").into(),
        provenance,
    };
    current(image, config).await?;
    let existing = api.get_opt(&name).await?;
    if let Some(existing) = &existing {
        ensure!(
            crate::reconcile::same_artifact(&existing.spec, &candidate)
                && existing.spec.request_digest == candidate.request_digest
                && existing.metadata.deletion_timestamp.is_none(),
            "VersionConflict"
        );
        if existing.status.as_ref().is_some_and(|s| s.verified) {
            return Ok(name);
        }
    }
    registry(config, &uid)?
        .publish(artifact, &config.cancel)
        .await?;
    current(image, config).await?;
    let release = match existing {
        Some(value) => value,
        None => {
            let mut resource = ImageRelease::new(&name, candidate.clone());
            resource.metadata.finalizers = Some(vec![crate::retention::PIN.into()]);
            match api.create(&PostParams::default(), &resource).await {
                Ok(value) => value,
                Err(kube::Error::Api(e)) if e.code == 409 => {
                    let value = api.get(&name).await?;
                    ensure!(
                        crate::reconcile::same_artifact(&value.spec, &candidate),
                        "VersionConflict"
                    );
                    value
                }
                Err(error) => return Err(error.into()),
            }
        }
    };
    current(image, config).await?;
    let status = ReleaseStatus {
        verified: true,
        observed_generation: release.metadata.generation.unwrap_or(1),
        conditions: vec![condition(
            release.metadata.generation.unwrap_or(1),
            true,
            "CompleteGraphReadback",
        )],
    };
    api.patch_status(
        &name,
        &PatchParams::default(),
        &Patch::Merge(
            json!({"metadata":{"resourceVersion":release.resource_version()},"status":status}),
        ),
    )
    .await?;
    Ok(name)
}
fn matching_snapshots(states: &[crate::build_state::State]) -> Result<()> {
    if let Some(first) = states.first() {
        for next in &states[1..] {
            ensure!(
                first.capture.ordinary_digest == next.capture.ordinary_digest
                    && first.capture.definition_digest == next.capture.definition_digest
                    && serde_json::to_value(&first.frozen.references)?
                        == serde_json::to_value(&next.frozen.references)?
                    && serde_json::to_value(&first.frozen.bindings)?
                        == serde_json::to_value(&next.frozen.bindings)?,
                "PlatformSourceSnapshotMismatch: publish a new version from stable inputs"
            );
        }
    }
    Ok(())
}
pub async fn reconcile(image: Arc<Image>, config: Arc<Config>) -> Result<Action> {
    image.spec.validate()?;
    let settings = config
        .direct
        .as_ref()
        .context("BuilderConfigurationRequired")?;
    let policy = configuration::load(
        config.client.clone(),
        &settings.system_namespace,
        &settings.installation,
    )
    .await?;
    let requested = configuration::requested(
        config.client.clone(),
        &settings.system_namespace,
        &settings.installation,
        &image,
    )
    .await?;
    let previous = image.status.clone().unwrap_or_default();
    // Channel/export changes advance the Image generation, but an immutable
    // artifact stays ready without creating fresh build demand or reading inputs.
    if policy.build_policy == configuration::BuildPolicy::OnDemand
        && requested.is_empty()
        && image.spec.explicit_platforms().is_none()
        && previous.observed_generation != image.metadata.generation.unwrap_or(1)
        && let Some(reference) = &previous.release_ref
    {
        ensure!(
            previous.platform_releases.len() <= 2
                && kimage_core::selector::dns_subdomain(reference),
            "InvalidPlatformReleaseStatus"
        );
        let releases: Api<ImageRelease> = Api::namespaced(config.client.clone(), &config.namespace);
        if let Some(release) = releases.get_opt(reference).await?
            && release.metadata.deletion_timestamp.is_none()
            && release.spec.image_uid == image.uid().context("MissingImageUid")?
            && release.spec.version == image.spec.version
            && release.spec.request_digest == crate::reconcile::request_digest(&image)?
            && release.status.as_ref().is_some_and(|s| {
                s.verified && s.observed_generation == release.metadata.generation.unwrap_or(1)
            })
        {
            let mut status = previous;
            status.conditions = vec![condition(
                image.metadata.generation.unwrap_or(1),
                true,
                "NativePlatformsRetained",
            )];
            checkpoint(&config, &image, status).await?;
            return Ok(Action::requeue(Duration::from_secs(30)));
        }
    }
    let platforms = configuration::plan(
        &image.spec,
        policy.build_policy,
        &available(&config).await?,
        &requested,
    );
    let mut status = image.status.clone().unwrap_or_default();
    if status.observed_generation != image.metadata.generation.unwrap_or(1) {
        status.release_ref = None;
        status.platform_releases.clear();
        status.conditions.clear();
        status.operation = None;
        status.build = None;
    }
    if platforms.is_empty() {
        if status.release_ref.is_none() {
            status.conditions = vec![condition(
                image.metadata.generation.unwrap_or(1),
                false,
                if requested.is_empty() && image.spec.explicit_platforms().is_none() {
                    "AwaitingDemand"
                } else {
                    "AwaitingNativePlatform"
                },
            )];
            checkpoint(&config, &image, status).await?;
        }
        return Ok(Action::requeue(Duration::from_secs(30)));
    }
    let images: Api<Image> = Api::namespaced(config.client.clone(), &config.namespace);
    let releases: Api<ImageRelease> = Api::namespaced(config.client.clone(), &config.namespace);
    let mut variants = Vec::new();
    let mut states = Vec::new();
    let mut pending = false;
    for platform in &platforms {
        let expected = child(&image, platform)?;
        let actual = match images.get_opt(&expected.name_any()).await? {
            Some(actual) => actual,
            None => match images.create(&PostParams::default(), &expected).await {
                Ok(actual) => actual,
                Err(kube::Error::Api(e)) if e.code == 409 => {
                    images.get(&expected.name_any()).await?
                }
                Err(error) => return Err(error.into()),
            },
        };
        validate_child(&image, &expected, &actual)?;
        let ready = actual.status.as_ref().filter(|s| {
            s.observed_generation == actual.metadata.generation.unwrap_or(1)
                && s.conditions
                    .iter()
                    .any(|c| c.type_ == "Ready" && c.status == "True")
        });
        let Some(reference) = ready.and_then(|s| s.release_ref.as_ref()) else {
            pending = true;
            continue;
        };
        let release = releases.get(reference).await?;
        ensure!(
            release.spec.image_uid == actual.uid().context("MissingImageUid")?
                && release.spec.version == image.spec.version
                && &release.spec.platform == platform
                && release.metadata.deletion_timestamp.is_none()
                && release.status.as_ref().is_some_and(|s| s.verified),
            "UnverifiedPlatformVariant"
        );
        states.push(
            crate::build_state::load(&actual, &config)
                .await?
                .context("MissingPlatformSnapshot")?,
        );
        variants.push(release);
    }
    if pending {
        if status.release_ref.is_none() {
            status.conditions = vec![condition(
                image.metadata.generation.unwrap_or(1),
                false,
                "BuildingNativePlatforms",
            )];
            checkpoint(&config, &image, status).await?;
        }
        return Ok(Action::requeue(Duration::from_secs(3)));
    }
    matching_snapshots(&states)?;
    // Retained variants are immutable. Once their complete graph was read back,
    // ordinary watch/resync events need no repeated registry download.
    let mut retained = true;
    for variant in &variants {
        let name = platform_release_name(
            &image.uid().context("MissingImageUid")?,
            &image.spec.version,
            &variant.spec.platform,
        );
        let existing = releases.get_opt(&name).await?;
        match existing {
            Some(existing)
                if existing.metadata.deletion_timestamp.is_none()
                    && existing.status.as_ref().is_some_and(|s| s.verified)
                    && existing.spec.root == variant.spec.root
                    && existing.spec.request_digest
                        == crate::reconcile::request_digest(&image)? =>
            {
                status
                    .platform_releases
                    .insert(variant.spec.platform.name(), name.clone());
                status.release_ref = Some(name);
            }
            _ => retained = false,
        }
    }
    if retained && variants.len() == 2 {
        let name = platform_release_name(
            &image.uid().context("MissingImageUid")?,
            &image.spec.version,
            &Platform::automatic(),
        );
        let children: Vec<_> = variants
            .iter()
            .map(|variant| {
                let mut root = variant.spec.root.clone();
                root.platform = Some(variant.spec.platform.clone());
                root
            })
            .collect();
        let expected = oci::descriptor(
            &serde_json::to_vec(
                &json!({"schemaVersion":2,"mediaType":oci::INDEX,"manifests":children}),
            )?,
            oci::INDEX,
        );
        retained = match releases.get_opt(&name).await? {
            Some(existing)
                if existing.metadata.deletion_timestamp.is_none()
                    && existing.status.as_ref().is_some_and(|s| s.verified)
                    && existing.spec.root == expected
                    && existing.spec.request_digest
                        == crate::reconcile::request_digest(&image)? =>
            {
                status.release_ref = Some(name);
                true
            }
            _ => false,
        };
    }
    if retained {
        status.conditions = vec![condition(
            image.metadata.generation.unwrap_or(1),
            true,
            "NativePlatformsPublished",
        )];
        checkpoint(&config, &image, status).await?;
        return Ok(Action::requeue(Duration::from_secs(30)));
    }
    let mut graphs = Vec::new();
    for variant in &variants {
        let graph = registry(&config, &variant.spec.image_uid)?
            .fetch_verified(&variant.spec.root, &variant.spec.platform, &config.cancel)
            .await?;
        let reference = publish(
            &image,
            &config,
            &graph,
            variant.spec.snapshot.clone(),
            variant.spec.provenance.clone(),
        )
        .await?;
        status
            .platform_releases
            .insert(variant.spec.platform.name(), reference.clone());
        status.release_ref = Some(reference);
        graphs.push(graph);
    }
    if graphs.len() == 2 {
        let graph =
            tokio::task::spawn_blocking(move || oci::aggregate(&graphs, oci::Limits::default()))
                .await??;
        status.release_ref = Some(
            publish(
                &image,
                &config,
                &graph,
                states[0].capture.ordinary_digest.clone(),
                None,
            )
            .await?,
        );
    }
    status.conditions = vec![condition(
        image.metadata.generation.unwrap_or(1),
        true,
        "NativePlatformsPublished",
    )];
    checkpoint(&config, &image, status).await?;
    Ok(Action::requeue(Duration::from_secs(30)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, http::StatusCode, response::IntoResponse};
    use serde_json::Value;
    use std::sync::Mutex;
    fn image() -> Image {
        serde_json::from_value(json!({"apiVersion":"kimage.dev/v1alpha1","kind":"Image","metadata":{"name":"service","namespace":"apps","uid":"parent-uid","resourceVersion":"1","generation":1},"spec":{"version":"1.0.0","source":{"build":{"format":"Containerfile","definition":{"inline":"FROM scratch\nCOPY message /message\n"},"execution":{"strategy":"Direct","builderRef":"default"},"inputs":{"contextPath":"/workspace/context","volumes":[{"name":"source","configMap":{"name":"source"}}],"volumeMounts":[{"name":"source","mountPath":"/workspace/context","readOnly":true}]}}}}})).unwrap()
    }
    #[test]
    fn automatic_policy_only_builds_requested_available_native_platforms() {
        use configuration::{BuildPolicy::*, plan};
        let mut image = image();
        let amd = BTreeSet::from([Platform::linux_amd64()]);
        let arm = BTreeSet::from([Platform::linux_arm64()]);
        let both = amd.union(&arm).cloned().collect();
        let empty = BTreeSet::new();
        for (policy, available, demand, expected) in [
            (OnDemand, &both, &empty, &empty),
            (OnDemand, &both, &amd, &amd),
            (OnDemand, &both, &arm, &arm),
            (OnDemand, &both, &both, &both),
            (OnDemand, &amd, &arm, &empty),
            (Eager, &both, &empty, &both),
            (Eager, &amd, &empty, &amd),
            (Eager, &arm, &empty, &arm),
            (Eager, &empty, &both, &empty),
        ] {
            assert_eq!(&plan(&image.spec, policy, available, demand), expected);
        }
        image.spec.platform = Platform::linux_arm64();
        assert_eq!(plan(&image.spec, OnDemand, &both, &empty), arm);
        assert!(plan(&image.spec, Eager, &amd, &empty).is_empty());
        image.spec.platform = Platform::automatic();
        image.spec.platforms = vec![Platform::linux_amd64()];
        assert_eq!(plan(&image.spec, Eager, &both, &arm), amd);
    }
    #[test]
    fn variant_identity_is_platform_and_version_keyed_and_refuses_unowned_or_edited_children() {
        let image = image();
        let amd = child(&image, &Platform::linux_amd64()).unwrap();
        let arm = child(&image, &Platform::linux_arm64()).unwrap();
        assert_ne!(amd.name_any(), arm.name_any());
        assert_eq!(amd.spec.platform, Platform::linux_amd64());
        validate_child(&image, &amd, &amd).unwrap();
        let mut altered = amd.clone();
        altered.metadata.owner_references = None;
        assert!(validate_child(&image, &amd, &altered).is_err());
        altered = amd.clone();
        altered.spec.version = "2.0.0".into();
        assert!(validate_child(&image, &amd, &altered).is_err());
        let mut newer = image.clone();
        newer.spec.version = "1.1.0".into();
        assert_ne!(
            child(&newer, &Platform::linux_amd64()).unwrap().name_any(),
            amd.name_any()
        );
    }
    #[test]
    fn aggregate_rejects_different_private_definition_input_or_volume_snapshots() {
        let source = serde_json::to_value(image().spec.source).unwrap()["build"].clone();
        let state:crate::build_state::State = serde_json::from_value(json!({
            "imageUid":"child","version":"1.0.0","generation":1,"snapshotId":"a".repeat(64),
            "requestDigest":oci::digest(b"request"),"fingerprint":"private",
            "frozen":{"build":source,"references":[{"kind":"ConfigMap","name":"source","uid":"source-uid","resourceVersion":"1"}],"bindings":[{"claim":"data","claimUid":"claim-uid","volume":"volume","volumeUid":"volume-uid"}]},
            "capture":{"ordinaryDigest":oci::digest(b"ordinary"),"definitionSensitive":true,"definitionDigest":oci::digest(b"private definition"),"files":1,"bytes":1},
            "workerUid":"native-worker","phase":"Published","root":null,"execution":null,
            "publishedFingerprint":"private","publishedReferences":[],"failedReason":null
        })).unwrap();
        matching_snapshots(&[state.clone(), state.clone()]).unwrap();
        let mut altered = state.clone();
        altered.capture.definition_digest = oci::digest(b"changed private definition");
        assert!(matching_snapshots(&[state.clone(), altered]).is_err());
        let mut altered = state.clone();
        altered.capture.ordinary_digest = oci::digest(b"changed ordinary input");
        assert!(matching_snapshots(&[state.clone(), altered]).is_err());
        let mut altered = state.clone();
        altered.frozen.references[0].resource_version = "2".into();
        assert!(matching_snapshots(&[state.clone(), altered]).is_err());
        let mut altered = state.clone();
        altered.frozen.bindings[0].volume_uid = "replacement-volume".into();
        assert!(matching_snapshots(&[state, altered]).is_err());
    }
    async fn controller_plan(
        policy: Option<configuration::BuildPolicy>,
        demand: BTreeSet<Platform>,
        expected_architectures: &[&str],
    ) {
        let original = image();
        let shared = Arc::new(Mutex::new((
            serde_json::to_value(&original).unwrap(),
            Vec::<Value>::new(),
        )));
        let state = shared.clone();
        let recorded = (!demand.is_empty()).then(|| {
            let mut resource = configuration::ImageDemand::new(
                "demand",
                configuration::Demand::for_image(&original, demand).unwrap(),
            );
            resource.metadata.labels = Some(BTreeMap::from([
                ("kimage.dev/installation".into(), "dev".into()),
                ("kimage.dev/role".into(), "platform-demand".into()),
            ]));
            serde_json::to_value(resource).unwrap()
        });
        let router = Router::new().fallback(move |request: axum::extract::Request| {
            let state = state.clone();
            let recorded = recorded.clone();
            async move {
                let path = request.uri().path().to_owned();
                let method = request.method().to_string();
                let bytes = axum::body::to_bytes(request.into_body(), 1024*1024).await.unwrap();
                let mut state = state.lock().unwrap();
                let missing = || (StatusCode::NOT_FOUND, Json(json!({"apiVersion":"v1","kind":"Status","status":"Failure","reason":"NotFound","code":404}))).into_response();
                let value = match (method.as_str(), path.as_str()) {
                    ("GET", "/api/v1/namespaces/system/configmaps/dev-config") => match policy {
                        Some(policy) => json!({"apiVersion":"v1","kind":"ConfigMap","metadata":{"labels":{"kimage.dev/installation":"dev"}},"data":{"configuration.json":serde_json::to_string(&configuration::Configuration {schema:1,build_policy:policy}).unwrap()}}),
                        None => return missing(),
                    },
                    ("GET", path) if path.contains("/imagedemands/") => match recorded {Some(v)=>v, None=>return missing()},
                    ("GET", "/api/v1/nodes") => json!({"apiVersion":"v1","kind":"NodeList","metadata":{},"items":(["amd64","arm64"].map(|arch|json!({"metadata":{"name":arch,"labels":{"kubernetes.io/os":"linux","kubernetes.io/arch":arch}},"status":{"conditions":[{"type":"Ready","status":"True"}]}})))}),
                    ("GET", "/apis/kimage.dev/v1alpha1/namespaces/apps/images/service") => state.0.clone(),
                    ("GET", path) if path.contains("/images/variant-") => return missing(),
                    ("POST", "/apis/kimage.dev/v1alpha1/namespaces/apps/images") => {
                        let mut value:Value=serde_json::from_slice(&bytes).unwrap();
                        state.1.push(value.clone());
                        value["metadata"]["uid"]=json!("child-uid");
                        value["metadata"]["generation"]=json!(1);
                        value
                    },
                    ("PATCH", "/apis/kimage.dev/v1alpha1/namespaces/apps/images/service/status") => {
                        state.0["status"] = serde_json::from_slice::<Value>(&bytes).unwrap()["status"].clone();
                        state.0.clone()
                    },
                    _ => panic!("unexpected platform controller API request: {method} {path}"),
                };
                Json(value).into_response()
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = kube::Client::try_from(kube::Config::new(
            format!("http://{}", listener.local_addr().unwrap())
                .parse()
                .unwrap(),
        ))
        .unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let config = Config {
            scope: None,
            direct: Some(crate::direct::Settings {
                architecture: None,
                system_namespace: "system".into(),
                installation: "dev".into(),
                builder_image: "unused".into(),
            }),
            client,
            namespace: "apps".into(),
            node: "unused".into(),
            input_image: "unused".into(),
            registry_origin: "https://unused.invalid/".parse().unwrap(),
            registry_ca: "unused".into(),
            publisher_password: "unused".into(),
            registry_pod: None,
            cancel: tokio_util::sync::CancellationToken::new(),
            concurrency: Arc::new(tokio::sync::Semaphore::new(1)),
        };
        reconcile(Arc::new(original), Arc::new(config))
            .await
            .unwrap();
        let state = shared.lock().unwrap();
        let actual: Vec<_> = state
            .1
            .iter()
            .map(|value| value["spec"]["platform"]["architecture"].as_str().unwrap())
            .collect();
        assert_eq!(actual, expected_architectures);
        assert_eq!(
            state.0["status"]["conditions"][0]["reason"],
            if expected_architectures.is_empty() {
                "AwaitingDemand"
            } else {
                "BuildingNativePlatforms"
            }
        );
        server.abort();
    }
    #[tokio::test]
    async fn controller_default_demand_and_configmap_eager_policy_control_real_api_build_creation()
    {
        controller_plan(None, BTreeSet::new(), &[]).await;
        controller_plan(
            Some(configuration::BuildPolicy::OnDemand),
            BTreeSet::from([Platform::linux_arm64()]),
            &["arm64"],
        )
        .await;
        controller_plan(
            Some(configuration::BuildPolicy::Eager),
            BTreeSet::new(),
            &["amd64", "arm64"],
        )
        .await;
        controller_plan(
            Some(configuration::BuildPolicy::OnDemand),
            BTreeSet::from([Platform::linux_amd64(), Platform::linux_arm64()]),
            &["amd64", "arm64"],
        )
        .await;
    }
}

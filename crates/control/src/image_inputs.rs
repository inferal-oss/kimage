//! Exact retained OCI dependencies, frozen once for all native attempts of a version.
use crate::{
    api::{Image, ImageRelease},
    reconcile::Config,
};
use anyhow::{Context, Result, ensure};
use k8s_openapi::api::core::v1::ConfigMap;
use kimage_core::{
    build::Build,
    oci::{self, Descriptor, Platform},
    selector::Selection,
};
use kube::{
    Api, ResourceExt,
    api::{ListParams, PostParams},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{collections::BTreeSet, path::Path};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FrozenImage {
    pub name: String,
    pub image: String,
    pub namespace: String,
    pub image_uid: String,
    pub release_uid: String,
    pub root: Descriptor,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Snapshot {
    schema: u32,
    declaration: String,
    inputs: Vec<FrozenImage>,
    signature: String,
}
fn snapshot_mac(
    snapshot: &Snapshot,
    owner: &Image,
    key: &[u8],
) -> Result<hmac::Hmac<sha2::Sha256>> {
    use hmac::Mac;
    ensure!(
        key.len() >= 16 && key.len() <= 4096,
        "ImageInputSnapshotKeyInvalid"
    );
    let mut mac =
        hmac::Hmac::<sha2::Sha256>::new_from_slice(key).context("ImageInputSnapshotKeyInvalid")?;
    mac.update(&serde_json::to_vec(&(
        "kimage-image-inputs-v1",
        owner.uid(),
        &owner.spec.version,
        snapshot.schema,
        &snapshot.declaration,
        &snapshot.inputs,
    ))?);
    Ok(mac)
}
pub(crate) fn validate(inputs: &[FrozenImage], build: &Build, namespace: &str) -> Result<()> {
    ensure!(
        inputs.len() == build.image_inputs.len(),
        "ImageInputSnapshotConflict"
    );
    for (input, declared) in inputs.iter().zip(&build.image_inputs) {
        let selector = declared.selector()?;
        ensure!(
            selector.namespace == namespace,
            "CrossNamespaceImageInputDenied"
        );
        ensure!(
            input.name == declared.name
                && input.image == declared.image
                && input.namespace == namespace
                && kimage_core::selector::dns_subdomain(&input.image_uid)
                && !input.release_uid.is_empty()
                && oci::valid_digest(&input.root.digest)
                && input.root.size > 0
                && input.root.size <= 2 * 1024 * 1024
                && matches!(input.root.media_type.as_str(), oci::MANIFEST | oci::INDEX),
            "ImageInputSnapshotConflict"
        );
    }
    Ok(())
}
async fn resolve(config: &Config, name: &str, reference: &str) -> Result<FrozenImage> {
    let selector = kimage_core::build::ImageInput {
        name: name.into(),
        image: reference.into(),
    }
    .selector()?;
    ensure!(
        selector.namespace == config.namespace,
        "CrossNamespaceImageInputDenied"
    );
    let Selection::Exact(version) = &selector.selection else {
        anyhow::bail!("ImageInputRequiresExactVersion")
    };
    let images: Api<Image> = Api::namespaced(config.client.clone(), &config.namespace);
    let releases: Api<ImageRelease> = Api::namespaced(config.client.clone(), &config.namespace);
    let before = images.get_opt(&selector.name).await?;
    ensure!(
        before
            .as_ref()
            .is_none_or(|i| i.metadata.deletion_timestamp.is_none()),
        "ImageInputDeleting"
    );
    let live_uid = before.as_ref().and_then(ResourceExt::uid);
    let mut candidates = Vec::new();
    let mut continuation: Option<String> = None;
    let mut count = 0;
    loop {
        let mut params = ListParams::default().limit(256);
        if let Some(token) = &continuation {
            params = params.continue_token(token);
        }
        let page = releases.list(&params).await?;
        count += page.items.len();
        ensure!(count <= 4096, "ImageInputCatalogTooLarge");
        candidates.extend(page.items.into_iter().filter(|r| {
            r.spec.image_name == selector.name
                && r.spec.version == version.to_string()
                && live_uid.as_ref().is_none_or(|uid| uid == &r.spec.image_uid)
                && r.metadata.deletion_timestamp.is_none()
                && r.status.as_ref().is_some_and(|s| {
                    s.verified && s.observed_generation == r.metadata.generation.unwrap_or(1)
                })
        }));
        continuation = page.metadata.continue_.filter(|s| !s.is_empty());
        if continuation.is_none() {
            break;
        }
    }
    let after = images.get_opt(&selector.name).await?;
    ensure!(
        before.as_ref().map(|i| (i.uid(), i.resource_version()))
            == after.as_ref().map(|i| (i.uid(), i.resource_version())),
        "ImageInputCatalogChangedRetry"
    );
    let uids: BTreeSet<_> = candidates.iter().map(|r| &r.spec.image_uid).collect();
    ensure!(uids.len() <= 1, "AmbiguousRetainedImageInput");
    // Prefer a complete aggregate, even for today's single-platform demand.
    // A later platform must use this same root, never resolve the name again.
    let release = candidates
        .into_iter()
        .max_by_key(|r| (r.spec.platform.is_automatic(), r.name_any()))
        .context("ImageInputNotPublished")?;
    Ok(FrozenImage {
        name: name.into(),
        image: reference.into(),
        namespace: config.namespace.clone(),
        image_uid: release.spec.image_uid,
        release_uid: release.metadata.uid.context("MissingReleaseUid")?,
        root: release.spec.root,
    })
}
fn publication_matches(owner: &Image, inputs: &[FrozenImage], release: &ImageRelease) -> bool {
    release.spec.image_uid == owner.uid().unwrap_or_default()
        && release.spec.image_name == owner.name_any()
        && release.spec.version == owner.spec.version
        && release.spec.platform.supported()
        && release.metadata.deletion_timestamp.is_none()
        && release.status.as_ref().is_some_and(|s| {
            s.verified && s.observed_generation == release.metadata.generation.unwrap_or(1)
        })
        && release.spec.provenance.as_ref().is_some_and(|p| {
            p.image_inputs.len() == inputs.len()
                && p.image_inputs.iter().zip(inputs).all(|(m, i)| {
                    m.name == i.name
                        && m.image == i.image
                        && m.image_uid == i.image_uid
                        && m.release_uid == i.release_uid
                        && m.root == i.root
                })
        })
}
async fn published_bindings(
    owner: &Image,
    inputs: &[FrozenImage],
    config: &Config,
) -> Result<bool> {
    // Uninstall removes service credentials, while verified release provenance
    // survives. Authenticate exact old bindings from that sealed publication;
    // never resolve a dependency name again or trust application ConfigMaps alone.
    let releases: Api<ImageRelease> = Api::namespaced(config.client.clone(), &config.namespace);
    let uid = owner.uid().context("MissingImageUid")?;
    for platform in [Platform::linux_amd64(), Platform::linux_arm64()] {
        let name =
            kimage_core::catalog::platform_release_name(&uid, &owner.spec.version, &platform);
        if let Some(release) = releases.get_opt(&name).await?
            && release.spec.platform == platform
            && publication_matches(owner, inputs, &release)
        {
            return Ok(true);
        }
    }
    Ok(false)
}
pub async fn freeze(image: &Image, config: &Config, build: &Build) -> Result<Vec<FrozenImage>> {
    if build.image_inputs.is_empty() {
        return Ok(Vec::new());
    }
    build.validate()?;
    // Platform children share the parent/version key. Verify that the parent
    // still owns the same declaration before using its snapshot.
    let images: Api<Image> = Api::namespaced(config.client.clone(), &config.namespace);
    let owner = if let Some(parent_uid) = image.labels().get("kimage.dev/platform-parent") {
        let reference = image
            .owner_references()
            .iter()
            .find(|r| r.kind == "Image" && &r.uid == parent_uid)
            .context("ImageInputParentConflict")?;
        let parent = images.get(&reference.name).await?;
        ensure!(
            parent.uid().as_ref() == Some(parent_uid)
                && parent.spec.version == image.spec.version
                && parent.metadata.deletion_timestamp.is_none(),
            "ImageInputParentConflict"
        );
        let crate::api::Source::Build(parent_build) = &parent.spec.source else {
            anyhow::bail!("ImageInputParentConflict")
        };
        ensure!(
            parent_build.image_inputs == build.image_inputs,
            "ImageInputParentConflict"
        );
        parent
    } else {
        image.clone()
    };
    let declaration = oci::digest(&serde_json::to_vec(&build.image_inputs)?);
    let key = format!(
        "imageinputs-{}",
        &oci::digest(&serde_json::to_vec(&(owner.uid(), &owner.spec.version))?)[7..47]
    );
    let maps: Api<ConfigMap> = Api::namespaced(config.client.clone(), &config.namespace);
    let map = match maps.get_opt(&key).await? {
        Some(map) => map,
        None => {
            let mut inputs = Vec::new();
            for input in &build.image_inputs {
                inputs.push(resolve(config, &input.name, &input.image).await?);
            }
            let snapshot = Snapshot {
                schema: 1,
                declaration: declaration.clone(),
                inputs,
                signature: String::new(),
            };
            use hmac::Mac;
            let mut snapshot = snapshot;
            snapshot.signature = hex::encode(
                snapshot_mac(
                    &snapshot,
                    &owner,
                    &std::fs::read(&config.publisher_password)?,
                )?
                .finalize()
                .into_bytes(),
            );
            let desired: ConfigMap = serde_json::from_value(
                json!({"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":key,"labels":{"kimage.dev/image-inputs":owner.uid()},"ownerReferences":[{"apiVersion":"kimage.dev/v1alpha1","kind":"Image","name":owner.name_any(),"uid":owner.uid(),"controller":false,"blockOwnerDeletion":false}]},"immutable":true,"data":{"inputs.json":serde_json::to_string(&snapshot)?}}),
            )?;
            match maps.create(&PostParams::default(), &desired).await {
                Ok(map) => map,
                Err(kube::Error::Api(e)) if e.code == 409 => maps.get(&key).await?,
                Err(error) => return Err(error.into()),
            }
        }
    };
    ensure!(
        map.immutable == Some(true)
            && map.metadata.deletion_timestamp.is_none()
            && map
                .owner_references()
                .iter()
                .any(|r| r.kind == "Image" && Some(r.uid.clone()) == owner.uid()),
        "ImageInputSnapshotOwnershipConflict"
    );
    let bytes = map
        .data
        .as_ref()
        .and_then(|d| d.get("inputs.json"))
        .context("ImageInputSnapshotMissing")?;
    ensure!(bytes.len() <= 32768, "ImageInputSnapshotTooLarge");
    let snapshot: Snapshot = serde_json::from_str(bytes)?;
    ensure!(
        snapshot.schema == 1 && snapshot.declaration == declaration,
        "ImageInputVersionConflict"
    );
    validate(&snapshot.inputs, build, &config.namespace)?;
    use hmac::Mac;
    let key = std::fs::read(&config.publisher_password)?;
    let mac = snapshot_mac(&snapshot, &owner, &key)?;
    let authenticated = hex::decode(&snapshot.signature)
        .ok()
        .is_some_and(|signature| mac.verify_slice(&signature).is_ok());
    ensure!(
        authenticated || published_bindings(&owner, &snapshot.inputs, config).await?,
        "ImageInputSnapshotAuthenticationFailed"
    );
    Ok(snapshot.inputs)
}
pub fn native_root(graph: &oci::VerifiedImage, platform: &Platform) -> Result<Descriptor> {
    ensure!(
        graph.platforms().contains(platform),
        "ImageInputPlatformUnavailable"
    );
    for manifest in graph
        .manifests()
        .iter()
        .filter(|d| d.media_type == oci::MANIFEST)
    {
        let value: serde_json::Value = serde_json::from_slice(&graph.bytes(manifest)?)?;
        let descriptor: Descriptor = serde_json::from_value(value["config"].clone())?;
        let config: serde_json::Value = serde_json::from_slice(&graph.bytes(&descriptor)?)?;
        if config["os"] == platform.os && config["architecture"] == platform.architecture {
            return Ok(manifest.clone());
        }
    }
    anyhow::bail!("ImageInputPlatformUnavailable")
}
pub fn validate_capture(
    inputs: &[FrozenImage],
    materials: &[kimage_core::build::ImageMaterial],
    platform: &Platform,
) -> Result<()> {
    ensure!(inputs.len() == materials.len(), "ImageInputCaptureMismatch");
    for (input, material) in inputs.iter().zip(materials) {
        ensure!(
            material.name == input.name
                && material.image == input.image
                && material.image_uid == input.image_uid
                && material.release_uid == input.release_uid
                && material.root == input.root
                && &material.platform == platform
                && material.manifest.media_type == oci::MANIFEST
                && oci::valid_digest(&material.manifest.digest),
            "ImageInputCaptureMismatch"
        );
    }
    Ok(())
}
pub fn write_layout(graph: &oci::VerifiedImage, destination: &Path) -> Result<()> {
    std::fs::create_dir(destination)?;
    std::fs::create_dir_all(destination.join("blobs/sha256"))?;
    std::fs::write(
        destination.join("oci-layout"),
        br#"{"imageLayoutVersion":"1.0.0"}"#,
    )?;
    std::fs::write(
        destination.join("index.json"),
        serde_json::to_vec(&json!({"schemaVersion":2,"manifests":[graph.root()]}))?,
    )?;
    let mut seen = BTreeSet::new();
    for descriptor in graph.blobs().iter().chain(graph.manifests()) {
        if !seen.insert(&descriptor.digest) {
            continue;
        }
        std::fs::hard_link(
            graph.path(descriptor)?,
            destination
                .join("blobs/sha256")
                .join(&descriptor.digest[7..]),
        )?;
    }
    Ok(())
}
pub async fn capture(
    session: &crate::containerfile::Session,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<Vec<kimage_core::build::ImageMaterial>> {
    if session.image_inputs.is_empty() {
        return Ok(Vec::new());
    }
    validate(&session.image_inputs, &session.build, &session.namespace)?;
    std::fs::create_dir("/snapshot/image-inputs")?;
    let ca = std::fs::read("/image-credentials/ca.crt")?;
    let password = std::fs::read_to_string("/image-credentials/reader-password")?;
    let mut total = 0_u64;
    let mut materials = Vec::new();
    for input in &session.image_inputs {
        let registry = crate::registry::Registry::new(
            session.registry_origin.parse()?,
            format!("{}/{}", input.namespace, input.image_uid),
            &ca,
            None,
            format!("reader/{}", session.namespace),
            password.trim_end().into(),
        )?;
        let graph = registry
            .fetch_verified(&input.root, &Platform::automatic(), cancel)
            .await?;
        let manifest = native_root(&graph, &session.platform)?;
        total += graph
            .blobs()
            .iter()
            .chain(graph.manifests())
            .map(|d| d.size)
            .sum::<u64>();
        ensure!(total <= 1024 * 1024 * 1024, "ImageInputBytesLimit");
        write_layout(
            &graph,
            &Path::new("/snapshot/image-inputs").join(&input.name),
        )?;
        materials.push(kimage_core::build::ImageMaterial {
            name: input.name.clone(),
            image: input.image.clone(),
            image_uid: input.image_uid.clone(),
            release_uid: input.release_uid.clone(),
            root: input.root.clone(),
            manifest,
            platform: session.platform.clone(),
        });
    }
    Ok(materials)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, http::StatusCode, response::IntoResponse};
    use std::sync::{Arc, Mutex};

    fn build() -> Build {
        serde_json::from_value(json!({"format":"Containerfile","definition":{"inline":"FROM toolchain AS build\nFROM toolchain\nCOPY --from=build /tool /tool\n"},"imageInputs":[{"name":"toolchain","image":"platform://apps/base:1.0.0"}],"inputs":{"contextPath":"/workspace/context"},"execution":{"strategy":"Direct","builderRef":"default"}})).unwrap()
    }
    fn graph(arch: &str) -> oci::VerifiedImage {
        let layer = vec![0_u8; 1024];
        let config = serde_json::to_vec(
            &json!({"os":"linux","architecture":arch,"rootfs":{"type":"layers","diff_ids":[oci::digest(&layer)]}}),
        )
        .unwrap();
        let manifest = serde_json::to_vec(&json!({"schemaVersion":2,"mediaType":oci::MANIFEST,"config":oci::descriptor(&config,oci::CONFIG),"layers":[oci::descriptor(&layer,oci::LAYER)]})).unwrap();
        let root = oci::descriptor(&manifest, oci::MANIFEST);
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut archive = tar::Builder::new(std::fs::File::create(file.path()).unwrap());
        let mut entries = vec![
            (
                "oci-layout".to_owned(),
                br#"{"imageLayoutVersion":"1.0.0"}"#.to_vec(),
            ),
            (
                "index.json".into(),
                serde_json::to_vec(&json!({"schemaVersion":2,"manifests":[root]})).unwrap(),
            ),
        ];
        for bytes in [config, manifest, layer] {
            entries.push((format!("blobs/sha256/{}", &oci::digest(&bytes)[7..]), bytes));
        }
        for (name, bytes) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o444);
            header.set_cksum();
            archive
                .append_data(&mut header, name, bytes.as_slice())
                .unwrap();
        }
        archive.finish().unwrap();
        drop(archive);
        oci::verify_archive(
            file.path(),
            &root,
            &Platform::linux(arch),
            oci::Limits::default(),
        )
        .unwrap()
    }
    #[test]
    fn complete_index_selects_exact_native_children_and_staged_bytes_survive_spool_removal() {
        let amd = graph("amd64");
        let arm = graph("arm64");
        let combined = oci::aggregate(&[amd, arm], oci::Limits::default()).unwrap();
        let a = native_root(&combined, &Platform::linux_amd64()).unwrap();
        let b = native_root(&combined, &Platform::linux_arm64()).unwrap();
        assert_ne!(a.digest, b.digest);
        let directory = tempfile::tempdir().unwrap();
        let layout = directory.path().join("base");
        write_layout(&combined, &layout).unwrap();
        let root = combined.root().clone();
        let descriptors = combined
            .blobs()
            .iter()
            .chain(combined.manifests())
            .cloned()
            .collect::<Vec<_>>();
        drop(combined);
        for descriptor in descriptors {
            let bytes =
                std::fs::read(layout.join("blobs/sha256").join(&descriptor.digest[7..])).unwrap();
            assert_eq!(bytes.len() as u64, descriptor.size);
            assert_eq!(oci::digest(&bytes), descriptor.digest);
        }
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(
                &std::fs::read(layout.join("index.json")).unwrap()
            )
            .unwrap()["manifests"][0]["digest"],
            root.digest
        );
        assert!(native_root(&graph("amd64"), &Platform::linux_arm64()).is_err());
    }
    #[test]
    fn transport_only_allows_exact_local_oci_identities_and_keeps_cache_controls() {
        let input = FrozenImage {
            name: "toolchain".into(),
            image: "platform://apps/base:1.0.0".into(),
            namespace: "apps".into(),
            image_uid: "base-uid".into(),
            release_uid: "release-uid".into(),
            root: oci::descriptor(b"index", oci::INDEX),
        };
        let mut session = crate::containerfile::Session {
            platform: Platform::linux_arm64(),
            build: build(),
            endpoint: "tcp://dev-builder-arm64.system.svc:1234".into(),
            tls_server_name: "dev-builder.system.svc".into(),
            worker_uid: "worker".into(),
            snapshot_id: "a".repeat(64),
            image_inputs: vec![input.clone()],
            namespace: "apps".into(),
            registry_origin: "https://dev-registry.system.svc:5443/".into(),
        };
        let capture = crate::input_snapshot::Capture {
            ordinary_digest: oci::digest(b"context"),
            definition_sensitive: false,
            definition_digest: oci::digest(b"recipe"),
            files: 0,
            bytes: 0,
            image_inputs: Vec::new(),
        };
        let args = crate::containerfile::arguments(&session, &capture).unwrap();
        assert!(args.contains(&"toolchain=/snapshot/image-inputs/toolchain".into()));
        assert!(args.contains(&format!(
            "context:toolchain=oci-layout://toolchain@{}",
            input.root.digest
        )));
        assert!(args.contains(&"force-network-mode=none".into()));
        let policy = crate::containerfile::source_policy(&session.image_inputs);
        let rules = policy["rules"].as_array().unwrap();
        assert_eq!(rules[0]["action"], "DENY");
        assert_eq!(
            rules.last().unwrap()["selector"]["identifier"],
            format!(
                "oci-layout://docker.io/library/toolchain@{}",
                input.root.digest
            )
        );
        assert!(
            rules
                .iter()
                .skip(1)
                .all(|r| r["selector"]["match_type"] == "EXACT")
        );
        assert!(
            !args
                .iter()
                .any(|a| a.contains("reader-password") || a.contains("publisher"))
        );
        session.build.cache.mode = kimage_core::build::CacheMode::Disabled;
        assert!(
            crate::containerfile::arguments(&session, &capture)
                .unwrap()
                .contains(&"--no-cache".into())
        );
        session.image_inputs[0].namespace = "other".into();
        assert!(crate::containerfile::arguments(&session, &capture).is_err());
    }
    #[tokio::test]
    async fn dependency_snapshot_is_shared_and_does_not_reresolve_a_recreated_base() {
        let original:Image=serde_json::from_value(json!({"apiVersion":"kimage.dev/v1alpha1","kind":"Image","metadata":{"name":"service","namespace":"apps","uid":"service-uid","generation":1},"spec":{"version":"1.0.0","source":{"build":build()}}})).unwrap();
        let shared = Arc::new(Mutex::new((
            None::<serde_json::Value>,
            0_usize,
            false,
            None::<serde_json::Value>,
        )));
        let state = shared.clone();
        let parent = original.clone();
        let router=Router::new().fallback(move |request:axum::extract::Request| {
            let state=state.clone();let parent=parent.clone();
            async move {
                let path=request.uri().path().to_owned();let method=request.method().to_string();
                let bytes=axum::body::to_bytes(request.into_body(),65536).await.unwrap();
                let mut state=state.lock().unwrap();
                let missing=||(StatusCode::NOT_FOUND,Json(json!({"apiVersion":"v1","kind":"Status","status":"Failure","reason":"NotFound","code":404}))).into_response();
                let value=match (method.as_str(),path.as_str()) {
                    ("GET","/apis/kimage.dev/v1alpha1/namespaces/apps/images/service")=>serde_json::to_value(parent).unwrap(),
                    ("GET","/apis/kimage.dev/v1alpha1/namespaces/apps/images/base")=>{
                        state.1+=1;let uid=if state.2 {"replacement-uid"}else{"base-uid"};
                        json!({"apiVersion":"kimage.dev/v1alpha1","kind":"Image","metadata":{"name":"base","uid":uid,"resourceVersion":"1"},"spec":{"version":"1.0.0","source":{"build":{"format":"Containerfile","definition":{"inline":"FROM scratch"},"inputs":{"contextPath":"/workspace/context"},"execution":{"strategy":"Direct","builderRef":"default"}}}}})
                    },
                    ("GET","/apis/kimage.dev/v1alpha1/namespaces/apps/imagereleases")=>{
                        let root=oci::descriptor(b"aggregate",oci::INDEX);
                        let release=json!({"apiVersion":"kimage.dev/v1alpha1","kind":"ImageRelease","metadata":{"name":"base-release","uid":"base-release-uid","generation":1},"spec":{"imageName":"base","imageUid":"base-uid","version":"1.0.0","snapshot":oci::digest(b"snapshot"),"root":root,"platform":Platform::automatic(),"requestDigest":oci::digest(b"request"),"sourceGeneration":1,"publisherVersion":"0.6.0"},"status":{"verified":true,"observedGeneration":1,"conditions":[]}});
                        json!({"apiVersion":"kimage.dev/v1alpha1","kind":"ImageReleaseList","metadata":{},"items":[release]})
                    },
                    ("GET",p) if p.contains("/imagereleases/release-") => match &state.3 { Some(value) => value.clone(), None => return missing() },
                    ("GET",p) if p.contains("/configmaps/imageinputs-")=>match &state.0 {Some(value)=>value.clone(),None=>return missing()},
                    ("POST","/api/v1/namespaces/apps/configmaps")=>{
                        assert!(state.0.is_none());let mut value:serde_json::Value=serde_json::from_slice(&bytes).unwrap();
                        value["metadata"]["uid"]=json!("snapshot-uid");state.0=Some(value.clone());value
                    },
                    _=>panic!("Unexpected {method} {path}"),
                };
                (StatusCode::OK,Json(value)).into_response()
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let client = kube::Client::try_from(kube::Config::new(
            format!("http://{address}").parse().unwrap(),
        ))
        .unwrap();
        let key = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(key.path(), b"fixture-publisher-key-never-sent-to-builder").unwrap();
        let config = Config {
            scope: None,
            direct: None,
            client,
            namespace: "apps".into(),
            node: "node".into(),
            input_image: "input".into(),
            registry_origin: "https://registry.example/".parse().unwrap(),
            registry_ca: "/unused".into(),
            publisher_password: key.path().into(),
            registry_pod: None,
            cancel: tokio_util::sync::CancellationToken::new(),
            concurrency: Arc::new(tokio::sync::Semaphore::new(1)),
        };
        let first = freeze(&original, &config, &build()).await.unwrap();
        assert_eq!(first[0].image_uid, "base-uid");
        assert_eq!(first[0].root.media_type, oci::INDEX);
        shared.lock().unwrap().2 = true;
        let mut child = original.clone();
        child.metadata.name = Some("child".into());
        child.metadata.uid = Some("child-uid".into());
        child.metadata.labels = Some(std::collections::BTreeMap::from([(
            "kimage.dev/platform-parent".into(),
            "service-uid".into(),
        )]));
        child.metadata.owner_references = Some(vec![
            k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference {
                api_version: "kimage.dev/v1alpha1".into(),
                kind: "Image".into(),
                name: "service".into(),
                uid: "service-uid".into(),
                controller: Some(true),
                block_owner_deletion: Some(false),
            },
        ]);
        child.spec.platform = Platform::linux_arm64();
        assert_eq!(freeze(&child, &config, &build()).await.unwrap(), first);
        assert_eq!(
            shared.lock().unwrap().1,
            2,
            "Recreated dependencies must never be resolved for later platforms"
        );
        let mut changed = build();
        changed.image_inputs[0].image = "platform://apps/base:2.0.0".into();
        assert!(
            freeze(&original, &config, &changed)
                .await
                .unwrap_err()
                .to_string()
                .contains("ImageInputVersionConflict")
        );
        let mut foreign = build();
        foreign.image_inputs[0].image = "platform://other/base:1.0.0".into();
        assert!(validate(&first, &foreign, "apps").is_err());
        std::fs::write(key.path(), b"regenerated-publisher-key-after-reinstall").unwrap();
        assert!(
            freeze(&child, &config, &build())
                .await
                .unwrap_err()
                .to_string()
                .contains("ImageInputSnapshotAuthenticationFailed"),
            "Unpublished snapshots cannot authenticate after key loss"
        );
        let input = &first[0];
        let proof = json!({"apiVersion":"kimage.dev/v1alpha1","kind":"ImageRelease","metadata":{"name":"consumer-native","uid":"consumer-release-uid","generation":1},"spec":{"imageName":"service","imageUid":"service-uid","version":"1.0.0","snapshot":oci::digest(b"context"),"root":oci::descriptor(b"output",oci::MANIFEST),"platform":Platform::linux_amd64(),"requestDigest":oci::digest(b"request"),"sourceGeneration":1,"publisherVersion":"0.6.0","provenance":{"format":"Containerfile","adapterVersion":"kimage.dev/containerfile-v1","inputs":[],"imageInputs":[{"name":input.name,"image":input.image,"imageUid":input.image_uid,"releaseUid":input.release_uid,"root":input.root,"manifest":oci::descriptor(b"native",oci::MANIFEST),"platform":Platform::linux_amd64()}],"execution":{"strategy":"Direct","builder":"default","engine":"BuildKit","engineVersion":"0.33.0","adapterVersion":"kimage.dev/containerfile-v1","workerUid":"worker","engineWorkerId":"engine","engineOperationId":"operation","attemptId":"a".repeat(64),"platform":Platform::linux_amd64(),"cacheMode":"Local","cachedExecutionSteps":0,"executedSteps":1}}},"status":{"verified":true,"observedGeneration":1,"conditions":[]}});
        shared.lock().unwrap().3 = Some(proof.clone());
        assert_eq!(freeze(&child, &config, &build()).await.unwrap(), first);
        assert_eq!(
            shared.lock().unwrap().1,
            2,
            "Reinstall recovery must not resolve recreated dependency names"
        );
        for mutation in ["unverified", "stale", "owner", "version", "material"] {
            let mut altered = proof.clone();
            match mutation {
                "unverified" => altered["status"]["verified"] = json!(false),
                "stale" => altered["status"]["observedGeneration"] = json!(2),
                "owner" => altered["spec"]["imageUid"] = json!("foreign-owner"),
                "version" => altered["spec"]["version"] = json!("2.0.0"),
                _ => {
                    altered["spec"]["provenance"]["imageInputs"][0]["releaseUid"] =
                        json!("foreign-dependency-release")
                }
            }
            shared.lock().unwrap().3 = Some(altered);
            assert!(freeze(&child, &config, &build()).await.is_err());
        }
        shared.lock().unwrap().3 = Some(proof);
        {
            let mut state = shared.lock().unwrap();
            let map = state.0.as_mut().unwrap();
            let mut snapshot: serde_json::Value =
                serde_json::from_str(map["data"]["inputs.json"].as_str().unwrap()).unwrap();
            snapshot["inputs"][0]["root"]["digest"] = json!(oci::digest(b"forged dependency"));
            map["data"]["inputs.json"] = json!(snapshot.to_string());
        }
        assert!(
            freeze(&original, &config, &build())
                .await
                .unwrap_err()
                .to_string()
                .contains("ImageInputSnapshotAuthenticationFailed")
        );
        server.abort();
    }
}

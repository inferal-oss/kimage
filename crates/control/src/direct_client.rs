//! Fixed unprivileged input/session Pod. Authors cannot supply a Pod template.
use crate::{
    api::Image,
    build_inputs::{self, Frozen},
    containerfile::Session,
    direct::{Approved, Settings},
    reconcile::Config,
};
use anyhow::{Context, Result, ensure};
use k8s_openapi::api::core::v1::{ConfigMap, Pod, Secret};
use kube::{
    Api, ResourceExt,
    api::{AttachParams, PostParams},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub frozen: Frozen,
    pub session: Session,
    pub request_digest: String,
    pub generation: i64,
}
pub fn pod(image: &Image, config: &Config, settings: &Settings, record: &Record) -> Result<Value> {
    let id = &record.session.snapshot_id;
    let mut metadata = build_inputs::object_metadata(image, config, id, "client")?;
    metadata["labels"]["kimage.dev/role"] = json!("builder-client");
    metadata["labels"]["kimage.dev/build-log-image"] = json!(
        image
            .labels()
            .get("kimage.dev/platform-parent")
            .cloned()
            .or_else(|| image.uid())
            .context("MissingImageUid")?
    );
    metadata["annotations"]["kimage.dev/build-snapshot-id"] = json!(id);
    metadata["annotations"]["kimage.dev/build-version"] = json!(image.spec.version);
    metadata["annotations"]["kimage.dev/build-platform"] = json!(record.session.platform.name());
    let mut volumes = vec![
        json!({"name":"snapshot","emptyDir":{"sizeLimit":"2Gi"}}),
        json!({"name":"sensitive","emptyDir":{"medium":"Memory","sizeLimit":"64Mi"}}),
        json!({"name":"configuration","configMap":{"name":format!("build-{}-session",&id[..20]),"defaultMode":292}}),
        json!({"name":"builder-credentials","secret":{"secretName":format!("build-{}-credentials",&id[..20]),"defaultMode":292}}),
    ];
    let mut mounts = vec![
        json!({"name":"snapshot","mountPath":"/snapshot"}),
        json!({"name":"sensitive","mountPath":"/sensitive"}),
        json!({"name":"configuration","mountPath":"/configuration","readOnly":true}),
        json!({"name":"builder-credentials","mountPath":"/builder-credentials","readOnly":true}),
    ];
    if !record.session.image_inputs.is_empty() {
        volumes[0]["emptyDir"]["sizeLimit"] = json!("4Gi");
        mounts.push(json!({"name":"snapshot","mountPath":"/tmp"}));
        volumes.push(json!({"name":"image-credentials","secret":{"secretName":format!("build-{}-image-credentials",&id[..20]),"defaultMode":292}}));
        mounts.push(
            json!({"name":"image-credentials","mountPath":"/image-credentials","readOnly":true}),
        );
    }
    for (index, volume) in record.frozen.build.inputs.volumes.iter().enumerate() {
        let name = format!("input-{index}");
        let value = if let Some(source) = volume.config_map.as_ref().or(volume.secret.as_ref()) {
            let items = source.items.as_ref().map(|items| {
                items
                    .iter()
                    .map(|i| json!({"key":i.key,"path":i.path,"mode":292}))
                    .collect::<Vec<_>>()
            });
            let mut value = json!({"defaultMode":292});
            value[if volume.secret.is_some() {
                "secretName"
            } else {
                "name"
            }] = json!(source.name);
            if let Some(items) = items {
                value["items"] = json!(items);
            }
            json!({"name":name,if volume.secret.is_some(){"secret"}else{"configMap"}:value})
        } else {
            json!({"name":name,"persistentVolumeClaim":{"claimName":volume.persistent_volume_claim.as_ref().context("MissingSourceVolume")?.claim_name,"readOnly":true}})
        };
        volumes.push(value);
        for mount in record
            .frozen
            .build
            .inputs
            .volume_mounts
            .iter()
            .filter(|m| m.name == volume.name)
        {
            let mut value = json!({"name":name,"mountPath":mount.mount_path,"readOnly":true});
            if let Some(sub) = &mount.sub_path {
                value["subPath"] = json!(sub);
            }
            mounts.push(value);
        }
    }
    let secure = json!({"runAsUser":65532,"runAsGroup":65532,"runAsNonRoot":true,"privileged":false,"allowPrivilegeEscalation":false,"readOnlyRootFilesystem":true,"capabilities":{"drop":["ALL"]}});
    let mut prepare = secure.clone();
    prepare["runAsUser"] = json!(0);
    prepare["runAsGroup"] = json!(0);
    prepare["runAsNonRoot"] = json!(false);
    let mut value = json!({"apiVersion":"v1","kind":"Pod","metadata":metadata,"spec":{"hostNetwork":false,"hostPID":false,"hostIPC":false,"nodeName":config.node,"automountServiceAccountToken":false,"restartPolicy":"Never","securityContext":{"seccompProfile":{"type":"RuntimeDefault"}},"initContainers":[{"name":"workspace","image":config.input_image,"imagePullPolicy":"IfNotPresent","command":["/kimage-input","prepare-build"],"securityContext":prepare,"resources":{"requests":{"cpu":"10m","memory":"16Mi"},"limits":{"cpu":"100m","memory":"64Mi"}},"volumeMounts":[{"name":"snapshot","mountPath":"/snapshot"},{"name":"sensitive","mountPath":"/sensitive"}]}],"containers":[{"name":"client","image":settings.builder_image,"imagePullPolicy":"IfNotPresent","command":["/snapshot/session","build-session"],"securityContext":secure,"resources":{"requests":{"cpu":"50m","memory":"64Mi"},"limits":{"cpu":"1","memory":"512Mi","ephemeral-storage":"2Gi"}},"readinessProbe":{"exec":{"command":["/snapshot/session","build-ready"]},"periodSeconds":2,"timeoutSeconds":2},"volumeMounts":mounts}],"volumes":volumes}});
    if !record.session.image_inputs.is_empty() {
        value["spec"]["containers"][0]["resources"]["limits"]["ephemeral-storage"] = json!("4Gi");
    }
    Ok(value)
}
pub async fn create(
    image: &Image,
    config: &Config,
    settings: &Settings,
    approved: &Approved,
    frozen: Frozen,
    id: &str,
) -> Result<Record> {
    let record = Record {
        session: Session {
            platform: image.spec.platform.clone(),
            build: frozen.build.clone(),
            endpoint: approved.endpoint.clone(),
            tls_server_name: approved.tls_server_name.clone(),
            worker_uid: approved.pod.uid().context("MissingWorkerUid")?,
            snapshot_id: id.into(),
            image_inputs: frozen.image_inputs.clone(),
            namespace: config.namespace.clone(),
            registry_origin: config.registry_origin.to_string(),
        },
        frozen,
        request_digest: crate::reconcile::request_digest(image)?,
        generation: image.metadata.generation.unwrap_or(1),
    };
    let maps: Api<ConfigMap> = Api::namespaced(config.client.clone(), &config.namespace);
    let secrets: Api<Secret> = Api::namespaced(config.client.clone(), &config.namespace);
    let map: ConfigMap = serde_json::from_value(
        json!({"apiVersion":"v1","kind":"ConfigMap","metadata":build_inputs::object_metadata(image,config,id,"session")?,"immutable":true,"data":{"session.json":serde_json::to_string(&record.session)?,"record.json":serde_json::to_string(&record)?}}),
    )?;
    ensure!(
        serde_json::to_vec(&map)?.len() <= 1_400_000,
        "BuildSessionMetadataTooLarge"
    );
    maps.create(&PostParams::default(), &map).await?;
    let mut secret: Secret = serde_json::from_value(
        json!({"apiVersion":"v1","kind":"Secret","metadata":build_inputs::object_metadata(image,config,id,"credentials")?,"immutable":true,"type":"Opaque"}),
    )?;
    secret.data = Some(approved.credentials.clone());
    secrets.create(&PostParams::default(), &secret).await?;
    if !record.session.image_inputs.is_empty() {
        let system_secrets: Api<Secret> =
            Api::namespaced(config.client.clone(), &settings.system_namespace);
        let reader = system_secrets
            .get(&format!("{}-proxy-credentials", settings.installation))
            .await?;
        ensure!(
            reader.labels().get(crate::install::LABEL) == Some(&settings.installation),
            "ImageInputCredentialOwnershipConflict"
        );
        let data = reader.data.context("ImageInputReaderCredentialsMissing")?;
        let mut selected = std::collections::BTreeMap::new();
        for key in ["ca.crt", "reader-password"] {
            selected.insert(
                key.to_owned(),
                data.get(key)
                    .context("ImageInputReaderCredentialsMissing")?
                    .clone(),
            );
        }
        let mut secret: Secret = serde_json::from_value(
            json!({"apiVersion":"v1","kind":"Secret","metadata":build_inputs::object_metadata(image,config,id,"image-credentials")?,"immutable":true,"type":"Opaque"}),
        )?;
        secret.data = Some(selected);
        secrets.create(&PostParams::default(), &secret).await?;
    }
    let desired: Pod = serde_json::from_value(pod(image, config, settings, &record)?)?;
    let pods: Api<Pod> = Api::namespaced(config.client.clone(), &config.namespace);
    pods.create(&PostParams::default(), &desired).await?;
    Ok(record)
}
pub async fn record(image: &Image, config: &Config, id: &str) -> Result<Option<Record>> {
    ensure!(
        id.len() == 64 && id.bytes().all(|c| c.is_ascii_hexdigit()),
        "InvalidBuildSnapshotId"
    );
    let maps: Api<ConfigMap> = Api::namespaced(config.client.clone(), &config.namespace);
    let Some(map) = maps
        .get_opt(&format!("build-{}-session", &id[..20]))
        .await?
    else {
        return Ok(None);
    };
    ensure!(
        map.owner_references()
            .iter()
            .any(|o| o.kind == "Image" && Some(o.uid.clone()) == image.uid())
            && map
                .labels()
                .get(build_inputs::SNAPSHOT_LABEL)
                .is_some_and(|v| v == &id[..40]),
        "BuildSessionOwnershipConflict"
    );
    let bytes = map
        .data
        .as_ref()
        .and_then(|d| d.get("record.json"))
        .context("MissingBuildSessionRecord")?;
    ensure!(bytes.len() <= 786432, "BuildSessionMetadataTooLarge");
    let record: Record = serde_json::from_str(bytes)?;
    ensure!(
        record.session.snapshot_id == id
            && record.generation == image.metadata.generation.unwrap_or(1)
            && record.request_digest == crate::reconcile::request_digest(image)?,
        "StaleOperation"
    );
    ensure!(map.immutable == Some(true), "BuildSessionOwnershipConflict");
    let settings = config
        .direct
        .as_ref()
        .context("BuilderConfigurationRequired")?;
    let pods: Api<Pod> = Api::namespaced(config.client.clone(), &config.namespace);
    let existing = pods.get(&crate::direct::client_name(id)?).await?;
    let desired = pod(image, config, settings, &record)?;
    let actual = serde_json::to_value(&existing)?;
    ensure!(
        crate::reconcile::matches_declared(&desired["spec"], &actual["spec"]),
        "BuildSessionDefinitionConflict"
    );
    ensure!(
        existing.owner_references() == map.owner_references(),
        "BuildSessionOwnershipConflict"
    );
    Ok(Some(record))
}
pub async fn exec(config: &Config, id: &str, args: Vec<String>) -> Result<Vec<u8>> {
    let pods: Api<Pod> = Api::namespaced(config.client.clone(), &config.namespace);
    let name = crate::direct::client_name(id)?;
    let mut command = vec!["/snapshot/session".into()];
    command.extend(args);
    let mut process = pods
        .exec(
            &name,
            command,
            &AttachParams::default().container("client").stderr(false),
        )
        .await?;
    let status = process.take_status().context("MissingBuildClientStatus")?;
    let mut output = process.stdout().context("MissingBuildClientOutput")?;
    let mut bytes = Vec::new();
    let read = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        (&mut output).take(65537).read_to_end(&mut bytes),
    )
    .await
    .context("BuildClientDeadline")??;
    ensure!(read <= 65536, "BuildClientMetadataTooLarge");
    drop(output);
    process.join().await?;
    ensure!(
        status
            .await
            .context("MissingBuildClientStatus")?
            .status
            .as_deref()
            == Some("Success"),
        "BuildClientCommandFailed"
    );
    Ok(bytes)
}

//! API-frozen object versions and guarded same-namespace PVC provenance.
use crate::{api::Image, reconcile::Config};
use anyhow::{Context, Result, ensure};
use k8s_openapi::{
    ByteString,
    api::core::v1::{ConfigMap, Node, PersistentVolume, PersistentVolumeClaim, Secret},
};
use kimage_core::build::{Build, ObjectInput};
use kube::{
    Api, ResourceExt,
    api::{DeleteParams, ListParams, PostParams, Preconditions},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub const SNAPSHOT_LABEL: &str = "kimage.dev/build-snapshot";
pub const INSTALLATION_LABEL: &str = "kimage.dev/build-installation";
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reference {
    pub kind: String,
    pub name: String,
    pub uid: String,
    pub resource_version: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Binding {
    pub claim: String,
    pub claim_uid: String,
    pub volume: String,
    pub volume_uid: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Frozen {
    pub build: Build,
    pub references: Vec<Reference>,
    pub bindings: Vec<Binding>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub image_inputs: Vec<crate::image_inputs::FrozenImage>,
}
fn metadata(image: &Image, id: &str, name: &str) -> Result<Value> {
    ensure!(
        id.len() == 64 && id.bytes().all(|c| c.is_ascii_hexdigit()),
        "InvalidBuildSnapshotId"
    );
    Ok(
        json!({"name":name,"labels":{SNAPSHOT_LABEL:&id[..40],"kimage.dev/image-uid":image.uid()},"ownerReferences":[{"apiVersion":"kimage.dev/v1alpha1","kind":"Image","name":image.name_any(),"uid":image.uid().context("MissingImageUid")?,"controller":true,"blockOwnerDeletion":false}]}),
    )
}
pub(crate) fn object_metadata(
    image: &Image,
    config: &Config,
    id: &str,
    suffix: &str,
) -> Result<Value> {
    ensure!(
        id.len() == 64 && id.bytes().all(|c| c.is_ascii_hexdigit()),
        "InvalidBuildSnapshotId"
    );
    let mut result = metadata(image, id, &format!("build-{}-{suffix}", &id[..20]))?;
    result["labels"][INSTALLATION_LABEL] = json!(
        config
            .direct
            .as_ref()
            .context("BuilderConfigurationRequired")?
            .installation
    );
    Ok(result)
}
fn selected(
    source: &ObjectInput,
    contents: &BTreeMap<String, Vec<u8>>,
) -> Result<BTreeMap<String, ByteString>> {
    let keys: Vec<_> = if let Some(items) = &source.items {
        items.iter().map(|item| item.key.clone()).collect()
    } else {
        contents.keys().cloned().collect()
    };
    ensure!(keys.len() <= 256, "UnsupportedInputKeyCount");
    let mut data = BTreeMap::new();
    let mut size = 0_usize;
    for key in keys {
        ensure!(
            !key.is_empty()
                && key.len() <= 253
                && !key.starts_with("..")
                && key != "."
                && key
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c)),
            "InvalidProjectedInputKey"
        );
        let value = contents.get(&key).context("MissingInputKey")?;
        size += value.len();
        ensure!(size <= 1024 * 1024, "InputObjectTooLarge");
        data.insert(key, ByteString(value.clone()));
    }
    Ok(data)
}
fn reference<T: ResourceExt>(kind: &str, object: &T) -> Result<Reference> {
    Ok(Reference {
        kind: kind.into(),
        name: object.name_any(),
        uid: object.uid().context("MissingInputUid")?,
        resource_version: object
            .resource_version()
            .context("MissingInputResourceVersion")?,
    })
}
async fn binding(config: &Config, claim: &str) -> Result<(Binding, Vec<Reference>)> {
    let claims: Api<PersistentVolumeClaim> =
        Api::namespaced(config.client.clone(), &config.namespace);
    let pvc = claims.get(claim).await?;
    ensure!(
        pvc.status.as_ref().and_then(|s| s.phase.as_deref()) == Some("Bound"),
        "SourcePvcNotBound"
    );
    let spec = pvc.spec.as_ref().context("MissingSourceClaimSpec")?;
    ensure!(
        matches!(spec.volume_mode.as_deref(), None | Some("Filesystem")),
        "UnsupportedSourceVolumeMode"
    );
    let name = spec.volume_name.as_ref().context("SourceVolumeUnknown")?;
    let volumes: Api<PersistentVolume> = Api::all(config.client.clone());
    let volume = volumes.get(name).await?;
    let spec = volume.spec.as_ref().context("MissingSourceVolumeSpec")?;
    ensure!(
        spec.claim_ref.as_ref().is_some_and(|r| r.uid == pvc.uid()
            && r.name.as_deref() == Some(claim)
            && r.namespace.as_deref() == Some(&config.namespace)),
        "SourceVolumeBindingMismatch"
    );
    if let Some(required) = spec
        .node_affinity
        .as_ref()
        .and_then(|a| a.required.as_ref())
    {
        let nodes: Api<Node> = Api::all(config.client.clone());
        let node = nodes.get(&config.node).await?;
        let mut eligible = false;
        for term in &required.node_selector_terms {
            let mut matches = true;
            let mut constraints = 0;
            for expression in term.match_expressions.iter().flatten() {
                constraints += 1;
                ensure!(
                    expression.operator == "In",
                    "UnsupportedSourceTopologyOperator"
                );
                matches &= node.labels().get(&expression.key).is_some_and(|value| {
                    expression
                        .values
                        .as_ref()
                        .is_some_and(|values| values.contains(value))
                });
            }
            for field in term.match_fields.iter().flatten() {
                constraints += 1;
                ensure!(
                    field.operator == "In" && field.key == "metadata.name",
                    "UnsupportedSourceTopologyField"
                );
                matches &= field
                    .values
                    .as_ref()
                    .is_some_and(|values| values.contains(&config.node));
            }
            eligible |= matches && constraints > 0;
        }
        ensure!(eligible, "UnsupportedInputPlacement");
    }
    Ok((
        Binding {
            claim: claim.into(),
            claim_uid: pvc.uid().context("MissingInputUid")?,
            volume: name.clone(),
            volume_uid: volume.uid().context("MissingInputUid")?,
        },
        vec![
            reference("PersistentVolumeClaim", &pvc)?,
            reference("PersistentVolume", &volume)?,
        ],
    ))
}
enum Original {
    Config(ConfigMap),
    Secret(Secret),
}
impl Original {
    fn contents(&self) -> Result<BTreeMap<String, Vec<u8>>> {
        let mut result = BTreeMap::new();
        match self {
            Self::Config(object) => {
                for (key, value) in object.data.iter().flatten() {
                    result.insert(key.clone(), value.as_bytes().to_vec());
                }
                for (key, value) in object.binary_data.iter().flatten() {
                    ensure!(!result.contains_key(key), "DuplicateInputKey");
                    result.insert(key.clone(), value.0.clone());
                }
            }
            Self::Secret(object) => {
                for (key, value) in object.data.iter().flatten() {
                    result.insert(key.clone(), value.0.clone());
                }
            }
        }
        Ok(result)
    }
    fn reference(&self) -> Result<Reference> {
        let labels = match self {
            Self::Config(object) => object.labels(),
            Self::Secret(object) => object.labels(),
        };
        ensure!(
            !labels.contains_key(SNAPSHOT_LABEL)
                && !labels.contains_key("kimage.dev/build-state")
                && !labels.contains_key("kimage.dev/build-log"),
            "ReservedControllerInputDenied"
        );
        match self {
            Self::Config(object) => reference("ConfigMap", object),
            Self::Secret(object) => reference("Secret", object),
        }
    }
}
pub async fn freeze(image: &Image, config: &Config, build: &Build, id: &str) -> Result<Frozen> {
    ensure!(
        id.len() == 64 && id.bytes().all(|c| c.is_ascii_hexdigit()),
        "InvalidBuildSnapshotId"
    );
    build.validate()?;
    let maps: Api<ConfigMap> = Api::namespaced(config.client.clone(), &config.namespace);
    let secrets: Api<Secret> = Api::namespaced(config.client.clone(), &config.namespace);
    let mut originals = BTreeMap::<(bool, String), Original>::new();
    let mut frozen = Frozen {
        build: build.clone(),
        references: Vec::new(),
        bindings: Vec::new(),
        image_inputs: crate::image_inputs::freeze(image, config, build).await?,
    };
    let mut pending = Vec::new();
    for (index, volume) in build.inputs.volumes.iter().enumerate() {
        if let Some(pvc) = &volume.persistent_volume_claim {
            crate::reconcile::ensure_no_writers(config, &pvc.claim_name).await?;
            let (binding, identity) = binding(config, &pvc.claim_name).await?;
            frozen.bindings.push(binding);
            frozen.references.extend(identity);
            continue;
        }
        let secret = volume.secret.is_some();
        let source = volume
            .secret
            .as_ref()
            .or(volume.config_map.as_ref())
            .context("InputVolumeSourceExclusive")?;
        let key = (secret, source.name.clone());
        if !originals.contains_key(&key) {
            let original = if secret {
                Original::Secret(secrets.get(&source.name).await?)
            } else {
                Original::Config(maps.get(&source.name).await?)
            };
            frozen.references.push(original.reference()?);
            originals.insert(key.clone(), original);
        }
        let contents = originals[&key].contents()?;
        let data = selected(source, &contents)?;
        let name = format!("build-{}-volume-{index}", &id[..20]);
        let object_metadata = object_metadata(image, config, id, &format!("volume-{index}"))?;
        let object = if secret {
            json!({"apiVersion":"v1","kind":"Secret","metadata":object_metadata,"immutable":true,"type":"Opaque","data":data})
        } else {
            json!({"apiVersion":"v1","kind":"ConfigMap","metadata":object_metadata,"immutable":true,"binaryData":data})
        };
        ensure!(
            serde_json::to_vec(&object)?.len() <= 1_400_000,
            "FrozenObjectRequestTooLarge"
        );
        pending.push((secret, object));
        let target = &mut frozen.build.inputs.volumes[index];
        target
            .secret
            .as_mut()
            .or(target.config_map.as_mut())
            .context("InputVolumeSourceExclusive")?
            .name = name;
    }
    // Validate every selection before materializing any copy. Each original API
    // object was fetched once even when several volumes select different keys.
    for (secret, object) in pending {
        if secret {
            let desired: Secret = serde_json::from_value(object)?;
            if let Some(existing) = secrets.get_opt(&desired.name_any()).await? {
                ensure!(
                    existing.immutable == Some(true)
                        && existing.data == desired.data
                        && existing.owner_references() == desired.owner_references(),
                    "FrozenInputOwnershipConflict"
                );
            } else {
                secrets.create(&PostParams::default(), &desired).await?;
            }
        } else {
            let desired: ConfigMap = serde_json::from_value(object)?;
            if let Some(existing) = maps.get_opt(&desired.name_any()).await? {
                ensure!(
                    existing.immutable == Some(true)
                        && existing.binary_data == desired.binary_data
                        && existing.owner_references() == desired.owner_references(),
                    "FrozenInputOwnershipConflict"
                );
            } else {
                maps.create(&PostParams::default(), &desired).await?;
            }
        }
    }
    Ok(frozen)
}
pub async fn unchanged(config: &Config, frozen: &Frozen) -> Result<()> {
    for reference in &frozen.references {
        let current = match reference.kind.as_str() {
            "ConfigMap" => {
                let api: Api<ConfigMap> = Api::namespaced(config.client.clone(), &config.namespace);
                let object = api.get(&reference.name).await?;
                self::reference("ConfigMap", &object)?
            }
            "Secret" => {
                let api: Api<Secret> = Api::namespaced(config.client.clone(), &config.namespace);
                let object = api.get(&reference.name).await?;
                self::reference("Secret", &object)?
            }
            "PersistentVolumeClaim" => {
                let api: Api<PersistentVolumeClaim> =
                    Api::namespaced(config.client.clone(), &config.namespace);
                let object = api.get(&reference.name).await?;
                self::reference("PersistentVolumeClaim", &object)?
            }
            "PersistentVolume" => {
                let api: Api<PersistentVolume> = Api::all(config.client.clone());
                self::reference("PersistentVolume", &api.get(&reference.name).await?)?
            }
            _ => anyhow::bail!("UnsupportedFrozenReference"),
        };
        ensure!(
            current.uid == reference.uid && current.resource_version == reference.resource_version,
            "InputChangedDuringBuild"
        );
    }
    for previous in &frozen.bindings {
        let (current, _) = binding(config, &previous.claim).await?;
        ensure!(
            current.claim_uid == previous.claim_uid
                && current.volume == previous.volume
                && current.volume_uid == previous.volume_uid,
            "SourceBindingChangedDuringCapture"
        );
    }
    Ok(())
}
pub async fn cleanup(image: &Image, config: &Config, id: &str) -> Result<()> {
    ensure!(
        id.len() == 64 && id.bytes().all(|c| c.is_ascii_hexdigit()),
        "InvalidBuildSnapshotId"
    );
    let selector = format!(
        "{SNAPSHOT_LABEL}={},kimage.dev/image-uid={}",
        &id[..40],
        image.uid().context("MissingImageUid")?
    );
    let params = ListParams::default().labels(&selector).limit(128);
    // Only this attempt's immutable metadata snapshots, never user inputs.
    let maps: Api<ConfigMap> = Api::namespaced(config.client.clone(), &config.namespace);
    let secrets: Api<Secret> = Api::namespaced(config.client.clone(), &config.namespace);
    let page = maps.list(&params).await?;
    ensure!(
        page.metadata
            .continue_
            .as_ref()
            .is_none_or(|s| s.is_empty()),
        "TooManyFrozenObjects"
    );
    for object in page {
        ensure!(
            object
                .name_any()
                .starts_with(&format!("build-{}-", &id[..20]))
                && object
                    .owner_references()
                    .iter()
                    .any(|o| Some(o.uid.clone()) == image.uid() && o.kind == "Image"),
            "FrozenCleanupOwnershipConflict"
        );
        let deletion = maps
            .delete(
                &object.name_any(),
                &DeleteParams {
                    preconditions: Some(Preconditions {
                        uid: object.uid(),
                        resource_version: object.resource_version(),
                    }),
                    ..Default::default()
                },
            )
            .await;
        match deletion {
            Ok(_) => {}
            Err(kube::Error::Api(error)) if error.code == 404 => {}
            Err(error) => return Err(error.into()),
        }
    }
    let page = secrets.list(&params).await?;
    ensure!(
        page.metadata
            .continue_
            .as_ref()
            .is_none_or(|s| s.is_empty()),
        "TooManyFrozenObjects"
    );
    for object in page {
        ensure!(
            object
                .name_any()
                .starts_with(&format!("build-{}-", &id[..20]))
                && object
                    .owner_references()
                    .iter()
                    .any(|o| Some(o.uid.clone()) == image.uid() && o.kind == "Image"),
            "FrozenCleanupOwnershipConflict"
        );
        let deletion = secrets
            .delete(
                &object.name_any(),
                &DeleteParams {
                    preconditions: Some(Preconditions {
                        uid: object.uid(),
                        resource_version: object.resource_version(),
                    }),
                    ..Default::default()
                },
            )
            .await;
        match deletion {
            Ok(_) => {}
            Err(kube::Error::Api(error)) if error.code == 404 => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

/// PVC producers must remain quiescent until the byte snapshot is complete.
pub async fn capture_quiescent(config: &Config, frozen: &Frozen) -> Result<()> {
    for binding in &frozen.bindings {
        crate::reconcile::ensure_no_writers(config, &binding.claim).await?;
    }
    unchanged(config, frozen).await
}

#[cfg(test)]
mod diagnostics_tests {
    use super::*;
    #[test]
    fn private_diagnostics_cannot_be_reused_as_build_inputs() {
        let secret:Secret=serde_json::from_value(json!({"metadata":{"name":"buildlog-private","uid":"uid","resourceVersion":"1","labels":{"kimage.dev/build-log":"image-uid"}},"data":{}})).unwrap();
        assert_eq!(
            Original::Secret(secret)
                .reference()
                .err()
                .unwrap()
                .to_string(),
            "ReservedControllerInputDenied"
        );
    }
}

//! Validated build declarations and engine-independent execution contracts.
//! No engine protocol, Containerfile parser, publication or Kubernetes client.
use crate::oci::{Platform, VerifiedImage};
use anyhow::{Result, ensure};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, future::Future, path::PathBuf};

pub const CONTEXT_ROOT: &str = "/workspace/context";
pub const INPUT_ROOT: &str = "/workspace/inputs";
pub const SECRET_ROOT: &str = "/run/kimage-build-secrets";
pub const DEFINITION_LIMIT: usize = 65536;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub enum Format {
    Containerfile,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub enum Strategy {
    Direct,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub enum CacheMode {
    #[default]
    Local,
    Disabled,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Cache {
    #[serde(default)]
    pub mode: CacheMode,
    /// Explicit non-sensitive cache key for commands depending on secret values.
    pub invalidation_token: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Execution {
    pub strategy: Strategy,
    pub builder_ref: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileDefinition {
    pub path: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Definition {
    Inline(String),
    FromFile(FileDefinition),
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KeyPath {
    pub key: String,
    pub path: String,
    pub mode: Option<u32>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObjectInput {
    pub name: String,
    pub items: Option<Vec<KeyPath>>,
    pub default_mode: Option<u32>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PvcInput {
    pub claim_name: String,
    #[serde(default)]
    pub read_only: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Volume {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_map: Option<ObjectInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<ObjectInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persistent_volume_claim: Option<PvcInput>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Mount {
    pub name: String,
    pub mount_path: String,
    #[serde(default)]
    pub read_only: bool,
    pub sub_path: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Inputs {
    pub context_path: String,
    #[serde(default)]
    pub volumes: Vec<Volume>,
    #[serde(default)]
    pub volume_mounts: Vec<Mount>,
    /// Required for any PVC; read-only consumers do not freeze a live writer.
    #[serde(default)]
    pub producer_handoff: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SecretBinding {
    pub id: String,
    pub source_file: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum NetworkMode {
    #[default]
    None,
}
/// A named, already published dependency. Resolution never starts another build.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageInput {
    pub name: String,
    pub image: String,
}
impl ImageInput {
    pub fn selector(&self) -> Result<crate::selector::Selector> {
        use crate::selector::Selection;
        let mut selector = crate::selector::Selector::parse(&self.image)?;
        if let Selection::Range(requirement) = &selector.selection
            && let [comparator] = requirement.comparators.as_slice()
            && comparator.op == semver::Op::Exact
            && let (Some(minor), Some(patch)) = (comparator.minor, comparator.patch)
        {
            let mut version = semver::Version::new(comparator.major, minor, patch);
            version.pre = comparator.pre.clone();
            selector.selection = Selection::Exact(version);
        }
        ensure!(
            matches!(selector.selection, Selection::Exact(_)),
            "ImageInputRequiresExactVersion"
        );
        Ok(selector)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Build {
    pub format: Format,
    pub definition: Definition,
    pub inputs: Inputs,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub image_inputs: Vec<ImageInput>,
    #[serde(default)]
    pub build_secrets: Vec<SecretBinding>,
    #[serde(default)]
    pub network_mode: NetworkMode,
    pub execution: Execution,
    #[serde(default)]
    pub cache: Cache,
}

pub fn relative(path: &str) -> Result<()> {
    ensure!(
        !path.is_empty()
            && path.len() <= 4096
            && path.split('/').all(|part| !part.is_empty()
                && part != "."
                && part != ".."
                && !part.contains(['\\', '\0'])
                && !part.chars().any(char::is_control)),
        "UnsafeInputPath"
    );
    Ok(())
}
pub fn absolute(path: &str) -> Result<()> {
    relative(
        path.strip_prefix('/')
            .ok_or_else(|| anyhow::anyhow!("InputPathMustBeAbsolute"))?,
    )
}
pub fn beneath(path: &str, root: &str) -> bool {
    path == root
        || path
            .strip_prefix(root)
            .is_some_and(|suffix| suffix.starts_with('/'))
}
fn mode(value: Option<u32>) -> Result<()> {
    ensure!(value.is_none_or(|v| v <= 0o777), "InvalidInputFileMode");
    Ok(())
}
fn validate_object(source: &ObjectInput) -> Result<()> {
    ensure!(
        crate::selector::dns_subdomain(&source.name),
        "InvalidInputObjectName"
    );
    mode(source.default_mode)?;
    if let Some(items) = &source.items {
        ensure!(
            !items.is_empty() && items.len() <= 256,
            "InvalidInputItemCount"
        );
        let mut paths = Vec::<&str>::new();
        let mut keys = std::collections::BTreeSet::new();
        for item in items {
            ensure!(
                !item.key.is_empty()
                    && item.key.len() <= 253
                    && item
                        .key
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c)),
                "InvalidInputKey"
            );
            ensure!(keys.insert(&item.key), "DuplicateInputKey");
            relative(&item.path)?;
            ensure!(
                !item.path.split('/').next().unwrap_or("").starts_with(".."),
                "ReservedProjectionPath"
            );
            mode(item.mode)?;
            ensure!(
                !paths
                    .iter()
                    .any(|p| beneath(p, &item.path) || beneath(&item.path, p)),
                "OverlappingInputItems"
            );
            paths.push(&item.path);
        }
    }
    Ok(())
}
impl Inputs {
    pub fn volume(&self, name: &str) -> Result<&Volume> {
        self.volumes
            .iter()
            .find(|v| v.name == name)
            .ok_or_else(|| anyhow::anyhow!("UnknownInputVolume"))
    }
    pub fn mount_for(&self, path: &str) -> Result<(&Mount, &Volume)> {
        absolute(path)?;
        let mount = self
            .volume_mounts
            .iter()
            .find(|m| beneath(path, &m.mount_path))
            .ok_or_else(|| anyhow::anyhow!("InputPathOutsideDeclaredMounts"))?;
        Ok((mount, self.volume(&mount.name)?))
    }
    pub fn validate(&self) -> Result<()> {
        absolute(&self.context_path)?;
        ensure!(
            beneath(&self.context_path, CONTEXT_ROOT),
            "UnsupportedContextRoot"
        );
        ensure!(
            self.volumes.len() <= 32 && self.volume_mounts.len() <= 32,
            "TooManyInputMounts"
        );
        let mut names = std::collections::BTreeSet::new();
        for volume in &self.volumes {
            ensure!(
                crate::selector::dns_label(&volume.name) && names.insert(&volume.name),
                "InvalidOrDuplicateInputVolume"
            );
            ensure!(
                usize::from(volume.config_map.is_some())
                    + usize::from(volume.secret.is_some())
                    + usize::from(volume.persistent_volume_claim.is_some())
                    == 1,
                "InputVolumeSourceExclusive"
            );
            for source in volume.config_map.iter().chain(volume.secret.iter()) {
                validate_object(source)?;
            }
            if let Some(pvc) = &volume.persistent_volume_claim {
                ensure!(
                    crate::selector::dns_subdomain(&pvc.claim_name),
                    "InvalidInputClaimName"
                );
                ensure!(
                    pvc.read_only && self.producer_handoff,
                    "PvcProducerHandoffRequired"
                );
            }
        }
        let mut destinations = Vec::<&str>::new();
        for mount in &self.volume_mounts {
            let source = self.volume(&mount.name)?;
            absolute(&mount.mount_path)?;
            ensure!(mount.read_only, "WritableInputMountDenied");
            if let Some(path) = &mount.sub_path {
                relative(path)?;
            }
            let roots: &[&str] = if source.secret.is_some() {
                &[SECRET_ROOT]
            } else {
                &[CONTEXT_ROOT, INPUT_ROOT]
            };
            ensure!(
                roots.iter().any(|root| beneath(&mount.mount_path, root)),
                "UnsupportedInputMountRoot"
            );
            ensure!(
                !destinations
                    .iter()
                    .any(|p| beneath(p, &mount.mount_path) || beneath(&mount.mount_path, p)),
                "OverlappingInputMounts"
            );
            destinations.push(&mount.mount_path);
        }
        ensure!(
            self.volumes
                .iter()
                .all(|v| self.volume_mounts.iter().any(|m| m.name == v.name)),
            "UnusedInputVolume"
        );
        Ok(())
    }
}
impl Build {
    pub fn validate(&self) -> Result<()> {
        self.inputs.validate()?;
        ensure!(self.image_inputs.len() <= 8, "TooManyImageInputs");
        let mut aliases = std::collections::BTreeSet::new();
        for input in &self.image_inputs {
            ensure!(input.image.len() <= 512, "ImageInputReferenceTooLong");
            ensure!(
                crate::selector::channel(&input.name)
                    && !matches!(input.name.as_str(), "scratch" | "context" | "dockerfile")
                    && aliases.insert(&input.name),
                "InvalidOrDuplicateImageInputName"
            );
            input.selector()?;
        }
        ensure!(
            crate::selector::dns_label(&self.execution.builder_ref),
            "InvalidBuilderReference"
        );
        ensure!(
            self.cache
                .invalidation_token
                .as_ref()
                .is_none_or(|v| v.len() <= 128
                    && v.bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))),
            "InvalidCacheInvalidationToken"
        );
        match &self.definition {
            Definition::Inline(text) => ensure!(
                !text.is_empty() && text.len() <= DEFINITION_LIMIT && !text.contains('\0'),
                "InvalidInlineDefinition"
            ),
            Definition::FromFile(file) => {
                self.inputs.mount_for(&file.path)?;
            }
        }
        ensure!(self.build_secrets.len() <= 32, "TooManyBuildSecrets");
        let mut ids = std::collections::BTreeSet::new();
        for secret in &self.build_secrets {
            ensure!(
                !secret.id.is_empty()
                    && secret.id != "."
                    && secret.id != ".."
                    && secret.id.len() <= 128
                    && secret
                        .id
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
                    && ids.insert(&secret.id),
                "InvalidOrDuplicateBuildSecretId"
            );
            let (_, volume) = self.inputs.mount_for(&secret.source_file)?;
            ensure!(volume.secret.is_some(), "BuildSecretRequiresSecretMount");
        }
        Ok(())
    }
}

/// Only selected ordinary bytes/modes belong in the public fingerprint.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ObjectIdentity {
    pub kind: String,
    pub name: String,
    pub uid: String,
    pub resource_version: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ImageMaterial {
    pub name: String,
    pub image: String,
    pub image_uid: String,
    pub release_uid: String,
    pub root: crate::oci::Descriptor,
    pub manifest: crate::oci::Descriptor,
    pub platform: Platform,
}
pub struct FrozenDefinition {
    pub format: Format,
    pub adapter_version: String,
    pub file: PathBuf,
    pub sensitive: bool,
}
pub struct FrozenRequest {
    pub definition: FrozenDefinition,
    pub ordinary_context: PathBuf,
    pub ordinary_digest: String,
    /// Random opaque identity, never a public hash of sensitive definition bytes.
    pub snapshot_id: String,
    pub secrets: BTreeMap<String, PathBuf>,
    pub platform: Platform,
    pub cache: Cache,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionReceipt {
    pub strategy: Strategy,
    pub builder: String,
    pub engine: String,
    pub engine_version: String,
    pub adapter_version: String,
    pub worker_uid: String,
    pub engine_worker_id: String,
    pub engine_operation_id: String,
    pub attempt_id: String,
    pub platform: Platform,
    pub cache_mode: CacheMode,
    pub cached_execution_steps: u64,
    pub executed_steps: u64,
}
pub struct ArtifactResult {
    pub artifact: VerifiedImage,
    pub execution: ExecutionReceipt,
}
pub trait Cancellation: Send + Sync {
    fn cancelled(&self) -> impl Future<Output = ()> + Send;
}
/// Implemented by strategies; format parsing and engine protocols stay separate.
pub trait Executor: Send + Sync {
    fn execute(
        &self,
        request: FrozenRequest,
        cancel: impl Cancellation,
    ) -> impl Future<Output = Result<ArtifactResult>> + Send;
    fn fence(&self, attempt_id: &str) -> impl Future<Output = Result<()>> + Send;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn fixture() -> Build {
        serde_json::from_value(json!({"format":"Containerfile","definition":{"fromFile":{"path":"/workspace/inputs/recipe/Containerfile"}},"inputs":{"contextPath":"/workspace/context","volumes":[{"name":"recipe","configMap":{"name":"recipe","items":[{"key":"recipe","path":"Containerfile","mode":292}]}},{"name":"secret","secret":{"name":"license"}}],"volumeMounts":[{"name":"recipe","mountPath":"/workspace/inputs/recipe","readOnly":true},{"name":"secret","mountPath":"/run/kimage-build-secrets/license","readOnly":true}]},"execution":{"strategy":"Direct","builderRef":"default"},"cache":{"mode":"Local"},"buildSecrets":[{"id":"license","sourceFile":"/run/kimage-build-secrets/license/value"}]})).unwrap()
    }
    #[test]
    fn named_image_inputs_require_exact_published_references_and_distinct_safe_aliases() {
        let mut build = fixture();
        for reference in [
            "platform://apps/base:1.2.3",
            "platform://apps/base?version==1.2.3",
            "platform://apps/base:1.2.3-rc.1",
        ] {
            build.image_inputs = vec![ImageInput {
                name: "toolchain".into(),
                image: reference.into(),
            }];
            build.validate().unwrap();
        }
        for reference in [
            "ubuntu:latest",
            "platform://apps/base:^1.0.0",
            "platform://apps/base:stable",
            "platform://apps/base:1.2",
            "platform://apps/base?version=1.2.3",
        ] {
            build.image_inputs[0].image = reference.into();
            assert!(build.validate().is_err(), "{reference}");
        }
        build.image_inputs[0].image = "platform://apps/base:1.2.3".into();
        for name in [
            "scratch",
            "context",
            "dockerfile",
            "123",
            "a,b",
            "../base",
            "Upper",
        ] {
            build.image_inputs[0].name = name.into();
            assert!(build.validate().is_err(), "{name}");
        }
        build.image_inputs[0].name = "toolchain".into();
        build.image_inputs.push(build.image_inputs[0].clone());
        assert!(build.validate().is_err());
        let legacy = serde_json::to_value(fixture()).unwrap();
        assert!(
            legacy.get("imageInputs").is_none(),
            "Empty additions must preserve old request digests"
        );
    }
    #[test]
    fn declaration_keeps_definition_context_and_secret_namespaces_distinct() {
        let mut build = fixture();
        build.validate().unwrap();
        build.definition = Definition::FromFile(FileDefinition {
            path: "/run/kimage-build-secrets/license/recipe".into(),
        });
        build.validate().unwrap();
        build.inputs.volume_mounts[1].mount_path = "/workspace/context/secrets".into();
        assert!(build.validate().is_err());
        let mut build = fixture();
        build.build_secrets[0].source_file = "/workspace/inputs/recipe/Containerfile".into();
        assert!(build.validate().is_err());
    }
    #[test]
    fn paths_modes_overlap_unions_and_unapproved_controls_are_rejected() {
        for path in [
            "/workspace/context/../host",
            "/workspace//context",
            "/workspace/context/./x",
            "relative",
            "/workspace/context/x\\y",
        ] {
            assert!(absolute(path).is_err());
        }
        for mutation in 0..5 {
            let mut build = fixture();
            match mutation {
                0 => build.inputs.volume_mounts[0].read_only = false,
                1 => build.inputs.volume_mounts[0].sub_path = Some("../escape".into()),
                2 => build.inputs.volume_mounts[0].mount_path = "/etc".into(),
                3 => build
                    .inputs
                    .volume_mounts
                    .push(build.inputs.volume_mounts[0].clone()),
                _ => {
                    build.inputs.volumes[0]
                        .config_map
                        .as_mut()
                        .unwrap()
                        .items
                        .as_mut()
                        .unwrap()[0]
                        .mode = Some(0o4755)
                }
            }
            assert!(build.validate().is_err());
        }
        let mut value = serde_json::to_value(fixture()).unwrap();
        value["definition"]["inline"] = json!("FROM scratch");
        assert!(serde_json::from_value::<Build>(value).is_err());
        let mut value = serde_json::to_value(fixture()).unwrap();
        value["execution"]["endpoint"] = json!("tcp://user-selected:1234");
        assert!(serde_json::from_value::<Build>(value).is_err());
        let mut value = serde_json::to_value(fixture()).unwrap();
        value["format"] = json!("Unapproved");
        assert!(serde_json::from_value::<Build>(value).is_err());
        let mut value = serde_json::to_value(fixture()).unwrap();
        value["inputs"]["volumeMounts"][0]["subPathExpr"] = json!("$(HOST)");
        assert!(serde_json::from_value::<Build>(value).is_err());
    }
}

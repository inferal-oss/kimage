use anyhow::{Result, ensure};
use kimage_core::{
    oci::{Descriptor, Platform},
    operation::Operation,
};
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(CustomResource, Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[kube(
    group = "kimage.dev",
    version = "v1alpha1",
    kind = "Image",
    namespaced,
    status = "ImageStatus",
    shortname = "kimg"
)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageSpec {
    pub version: String,
    #[serde(default)]
    pub channels: BTreeMap<String, String>,
    pub source: Source,
    #[serde(default = "Platform::automatic")]
    pub platform: Platform,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub platforms: Vec<Platform>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exports: Vec<crate::export_api::Target>,
}
impl ImageSpec {
    pub fn explicit_platforms(&self) -> Option<std::collections::BTreeSet<Platform>> {
        if !self.platforms.is_empty() {
            Some(self.platforms.iter().cloned().collect())
        } else if self.platform.supported() {
            Some(std::collections::BTreeSet::from([self.platform.clone()]))
        } else {
            None
        }
    }
    pub fn declared_platforms(&self) -> std::collections::BTreeSet<Platform> {
        self.explicit_platforms().unwrap_or_else(|| {
            std::collections::BTreeSet::from([Platform::linux_amd64(), Platform::linux_arm64()])
        })
    }
    pub fn validate(&self) -> Result<()> {
        crate::export_api::validate(&self.exports)?;
        kimage_core::selector::exact(&self.version)?;
        ensure!(self.channels.len() <= 32, "TooManyChannels");
        for (c, v) in &self.channels {
            ensure!(kimage_core::selector::channel(c), "InvalidChannel");
            kimage_core::selector::exact(v)?;
        }
        ensure!(
            (self.platform.supported() && self.platforms.is_empty())
                || (self.platform.is_automatic()
                    && self.platforms.len() <= 2
                    && self.platforms.iter().all(Platform::supported)
                    && self
                        .platforms
                        .iter()
                        .collect::<std::collections::BTreeSet<_>>()
                        .len()
                        == self.platforms.len()),
            "UnsupportedPlatform"
        );
        match &self.source {
            Source::Build(build) => build.validate()?,
            Source::OciArchive(a) => {
                ensure!(
                    kimage_core::selector::dns_subdomain(&a.claim_name),
                    "InvalidClaimName"
                );
                relative(&a.path)?;
                ensure!(
                    kimage_core::oci::valid_digest(&a.root.digest)
                        && a.root.size > 0
                        && matches!(
                            a.root.media_type.as_str(),
                            kimage_core::oci::MANIFEST | kimage_core::oci::INDEX
                        ),
                    "InvalidRootDescriptor"
                );
                ensure!(a.producer_handoff, "PvcProducerHandoffRequired");
            }
        }
        Ok(())
    }
}
/// Externally tagged enum enforces exactly one source during Rust decoding.
/// Unsupported build formats are added only with their milestone's typed schema.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Source {
    OciArchive(ArchiveSource),
    Build(kimage_core::build::Build),
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArchiveSource {
    pub claim_name: String,
    pub path: String,
    pub root: Descriptor,
    /// Producer promises no writes during capture. A read-only mount is insufficient.
    pub producer_handoff: bool,
}
pub fn relative(path: &str) -> Result<()> {
    ensure!(
        !path.is_empty()
            && path.len() <= 4096
            && path
                .split('/')
                .all(|s| !s.is_empty() && s != "." && s != ".." && !s.contains(['\\', '\0'])),
        "UnsafeRelativePath"
    );
    Ok(())
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImageStatus {
    pub observed_generation: i64,
    #[serde(default)]
    pub conditions: Vec<k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition>,
    pub operation: Option<Operation>,
    pub release_ref: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub platform_releases: BTreeMap<String, String>,
    pub build: Option<BuildStatus>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exports: Vec<crate::export_api::Status>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BuildStatus {
    /// Opaque random handle, not a fingerprint of sensitive definition bytes.
    pub snapshot_id: String,
    pub ordinary_digest: String,
    pub private_state_ref: String,
    pub execution: Option<kimage_core::build::ExecutionReceipt>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BuildProvenance {
    pub format: String,
    pub adapter_version: String,
    pub inputs: Vec<kimage_core::build::ObjectIdentity>,
    pub execution: kimage_core::build::ExecutionReceipt,
}
#[derive(CustomResource, Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[kube(
    group = "kimage.dev",
    version = "v1alpha1",
    kind = "ImageRelease",
    namespaced,
    status = "ReleaseStatus"
)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageReleaseSpec {
    pub image_name: String,
    pub image_uid: String,
    pub version: String,
    pub snapshot: String,
    pub root: Descriptor,
    pub platform: Platform,
    pub request_digest: String,
    pub source_generation: i64,
    pub publisher_version: String,
    pub provenance: Option<BuildProvenance>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseStatus {
    pub verified: bool,
    pub observed_generation: i64,
    #[serde(default)]
    pub conditions: Vec<k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition>,
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn union_and_paths_reject_unsupported_declarations() {
        for source in [
            json!({}),
            json!({"ociArchive":{},"build":{}}),
            json!({"externalRegistry":{}}),
        ] {
            assert!(serde_json::from_value::<Source>(source).is_err());
        }
        for path in ["../archive", "/archive", "a//b", "a/./b", "a\\b"] {
            assert!(relative(path).is_err());
        }
    }
}

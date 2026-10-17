use anyhow::{Result, ensure};
use kimage_core::{
    oci::{Descriptor, Platform},
    operation::Operation,
};
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(CustomResource, Clone, Debug, Serialize, JsonSchema)]
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
    /// Opaque additions survive older readers; they are never executed.
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: BTreeMap<String, Value>,
}
impl<'de> Deserialize<'de> for ImageSpec {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Fields {
            version: String,
            #[serde(default)]
            channels: BTreeMap<String, String>,
            source: Source,
            #[serde(default = "Platform::automatic")]
            platform: Platform,
            #[serde(default)]
            platforms: Vec<Platform>,
            #[serde(default)]
            exports: Vec<crate::export_api::Target>,
            #[serde(flatten)]
            extra: BTreeMap<String, Value>,
        }
        let f = Fields::deserialize(deserializer)?;
        Ok(Self {
            version: f.version,
            channels: f.channels,
            source: f.source,
            platform: f.platform,
            platforms: f.platforms,
            exports: f.exports,
            extra: f.extra,
        })
    }
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
        ensure!(self.extra.is_empty(), "UnsupportedImageFields");
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
            Source::Unsupported(_) => anyhow::bail!("UnsupportedImageSource"),
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
#[derive(Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Source {
    OciArchive(ArchiveSource),
    Build(kimage_core::build::Build),
    #[schemars(skip)]
    Unsupported(Value),
}
impl Serialize for Source {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Self::Build(v) => serde_json::json!({"build":v}).serialize(serializer),
            Self::OciArchive(v) => serde_json::json!({"ociArchive":v}).serialize(serializer),
            Self::Unsupported(v) => v.serialize(serializer),
        }
    }
}
impl<'de> Deserialize<'de> for Source {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let raw = Value::deserialize(deserializer)?;
        let fields = raw
            .as_object()
            .ok_or_else(|| serde::de::Error::custom("InvalidImageSource"))?;
        if fields.is_empty() || fields.contains_key("build") && fields.contains_key("ociArchive") {
            return Err(serde::de::Error::custom("SourceMustBeExclusive"));
        }
        if fields.len() != 1 {
            return Ok(Self::Unsupported(raw));
        }
        if let Some(v) = fields.get("build") {
            return Ok(serde_json::from_value(v.clone())
                .map(Self::Build)
                .unwrap_or(Self::Unsupported(raw)));
        }
        if let Some(v) = fields.get("ociArchive") {
            return Ok(serde_json::from_value(v.clone())
                .map(Self::OciArchive)
                .unwrap_or(Self::Unsupported(raw)));
        }
        Ok(Self::Unsupported(raw))
    }
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
#[derive(Clone, Debug, Default, JsonSchema)]
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
    #[schemars(skip)]
    pub raw: BTreeMap<String, Value>,
    #[schemars(skip)]
    pub opaque_state: bool,
}
// Read stable readiness separately from versioned execution journals. A newer
// journal must never poison an entire Image list or be reinterpreted as idle.
impl<'de> Deserialize<'de> for ImageStatus {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let raw = BTreeMap::<String, Value>::deserialize(deserializer)?;
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Stable {
            observed_generation: i64,
            #[serde(default)]
            conditions: Vec<k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition>,
            release_ref: Option<String>,
            #[serde(default)]
            platform_releases: BTreeMap<String, String>,
        }
        let stable: Stable =
            serde_json::from_value(serde_json::to_value(&raw).map_err(serde::de::Error::custom)?)
                .map_err(serde::de::Error::custom)?;
        let operation = serde_json::from_value::<Option<Operation>>(
            raw.get("operation").cloned().unwrap_or(Value::Null),
        );
        let build = serde_json::from_value::<Option<BuildStatus>>(
            raw.get("build").cloned().unwrap_or(Value::Null),
        );
        let exports = serde_json::from_value::<Vec<crate::export_api::Status>>(
            raw.get("exports").cloned().unwrap_or(serde_json::json!([])),
        );
        let opaque_state = operation.is_err() || build.is_err() || exports.is_err();
        Ok(Self {
            observed_generation: stable.observed_generation,
            conditions: stable.conditions,
            release_ref: stable.release_ref,
            platform_releases: stable.platform_releases,
            operation: operation.unwrap_or_default(),
            build: build.unwrap_or_default(),
            exports: exports.unwrap_or_default(),
            raw,
            opaque_state,
        })
    }
}
fn merge_reader_value(old: Value, new: Value) -> Value {
    match (old, new) {
        (Value::Object(mut a), Value::Object(b)) => {
            for (k, v) in b {
                let old = a.remove(&k).unwrap_or(Value::Null);
                a.insert(k, merge_reader_value(old, v));
            }
            Value::Object(a)
        }
        (Value::Array(a), Value::Array(b)) if a.len() == b.len() => Value::Array(
            a.into_iter()
                .zip(b)
                .map(|(a, b)| merge_reader_value(a, b))
                .collect(),
        ),
        (_, new) => new,
    }
}
impl ImageStatus {
    pub(crate) fn owned_patch(&self) -> Value {
        let mut value = serde_json::json!({"observedGeneration":self.observed_generation,"conditions":self.conditions,
            "operation":self.operation,"releaseRef":self.release_ref,"build":self.build});
        if !self.platform_releases.is_empty() {
            value["platformReleases"] = serde_json::json!(self.platform_releases)
        }
        if !self.exports.is_empty() {
            value["exports"] = serde_json::json!(self.exports)
        }
        value
    }
}
impl Serialize for ImageStatus {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let mut known = self.owned_patch();
        if self.opaque_state {
            for key in ["operation", "build", "exports"] {
                if let Some(value) = self.raw.get(key) {
                    known[key] = value.clone();
                }
            }
        }
        merge_reader_value(
            serde_json::to_value(&self.raw).map_err(serde::ser::Error::custom)?,
            known,
        )
        .serialize(serializer)
    }
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub image_inputs: Vec<kimage_core::build::ImageMaterial>,
    pub execution: kimage_core::build::ExecutionReceipt,
}
#[derive(CustomResource, Clone, Debug, JsonSchema)]
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
    #[schemars(skip)]
    pub extra: BTreeMap<String, Value>,
    #[schemars(skip)]
    pub raw_provenance: Option<Value>,
}
impl<'de> Deserialize<'de> for ImageReleaseSpec {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Fields {
            image_name: String,
            image_uid: String,
            version: String,
            snapshot: String,
            root: Descriptor,
            platform: Platform,
            request_digest: String,
            source_generation: i64,
            publisher_version: String,
            provenance: Option<Value>,
            #[serde(flatten)]
            extra: BTreeMap<String, Value>,
        }
        let f = Fields::deserialize(deserializer)?;
        let provenance = f
            .provenance
            .clone()
            .and_then(|v| serde_json::from_value(v).ok());
        Ok(Self {
            image_name: f.image_name,
            image_uid: f.image_uid,
            version: f.version,
            snapshot: f.snapshot,
            root: f.root,
            platform: f.platform,
            request_digest: f.request_digest,
            source_generation: f.source_generation,
            publisher_version: f.publisher_version,
            provenance,
            extra: f.extra,
            raw_provenance: f.provenance,
        })
    }
}
impl Serialize for ImageReleaseSpec {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let mut v = serde_json::json!({"imageName":self.image_name,"imageUid":self.image_uid,"version":self.version,
            "snapshot":self.snapshot,"root":self.root,"platform":self.platform,"requestDigest":self.request_digest,
            "sourceGeneration":self.source_generation,"publisherVersion":self.publisher_version,
            "provenance":self.raw_provenance.clone().unwrap_or_else(||serde_json::to_value(&self.provenance).expect("serializable provenance"))});
        v.as_object_mut().unwrap().extend(self.extra.clone());
        v.serialize(serializer)
    }
}
impl PartialEq for ImageReleaseSpec {
    fn eq(&self, other: &Self) -> bool {
        serde_json::to_value(self).expect("serializable release")
            == serde_json::to_value(other).expect("serializable release")
    }
}
impl Eq for ImageReleaseSpec {}
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
        for source in [json!({}), json!({"ociArchive":{},"build":{}})] {
            assert!(serde_json::from_value::<Source>(source).is_err());
        }
        for path in ["../archive", "/archive", "a//b", "a/./b", "a\\b"] {
            assert!(relative(path).is_err());
        }
    }
}

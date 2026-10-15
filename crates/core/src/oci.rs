//! Bounded disk spooling and complete single-platform OCI graph verification.
//! No archive member is extracted under an archive-controlled path.
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

pub const MANIFEST: &str = "application/vnd.oci.image.manifest.v1+json";
pub const INDEX: &str = "application/vnd.oci.image.index.v1+json";
pub const CONFIG: &str = "application/vnd.oci.image.config.v1+json";
pub const LAYER: &str = "application/vnd.oci.image.layer.v1.tar";
pub const GZIP_LAYER: &str = "application/vnd.oci.image.layer.v1.tar+gzip";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", from = "WireDescriptor")]
pub struct Descriptor {
    pub media_type: String,
    pub digest: String,
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<Platform>,
}
// Exporters attach ordinary OCI annotations. They do not alter graph identity;
// URL-backed/embedded descriptors remain unsupported rather than fetched.
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WireDescriptor {
    media_type: String,
    digest: String,
    size: u64,
    platform: Option<Platform>,
    #[serde(default, rename = "annotations")]
    _annotations: BTreeMap<String, String>,
}
impl From<WireDescriptor> for Descriptor {
    fn from(value: WireDescriptor) -> Self {
        Self {
            media_type: value.media_type,
            digest: value.digest,
            size: value.size,
            platform: value.platform,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Platform {
    pub os: String,
    pub architecture: String,
}
impl Platform {
    pub fn linux_amd64() -> Self {
        Self {
            os: "linux".into(),
            architecture: "amd64".into(),
        }
    }
}
#[derive(Clone, Copy)]
pub struct Limits {
    pub archive_bytes: u64,
    pub blob_bytes: u64,
    pub expanded_layer_bytes: u64,
    pub members: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            archive_bytes: 1024 * 1024 * 1024,
            blob_bytes: 512 * 1024 * 1024,
            expanded_layer_bytes: 2 * 1024 * 1024 * 1024,
            members: 4096,
        }
    }
}
pub fn valid_digest(digest: &str) -> bool {
    digest.strip_prefix("sha256:").is_some_and(|value| {
        value.len() == 64
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
pub fn digest(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}
pub fn descriptor(bytes: &[u8], media_type: &str) -> Descriptor {
    Descriptor {
        media_type: media_type.into(),
        digest: digest(bytes),
        size: bytes.len() as u64,
        platform: None,
    }
}

pub struct VerifiedImage {
    _spool: tempfile::TempDir,
    root: Descriptor,
    /// Topologically ordered child manifest(s), root last.
    manifests: Vec<Descriptor>,
    blobs: Vec<Descriptor>,
    paths: BTreeMap<String, PathBuf>,
}
impl VerifiedImage {
    pub fn root(&self) -> &Descriptor {
        &self.root
    }
    pub fn manifests(&self) -> &[Descriptor] {
        &self.manifests
    }
    pub fn blobs(&self) -> &[Descriptor] {
        &self.blobs
    }
    pub fn path(&self, descriptor: &Descriptor) -> Result<&Path> {
        self.paths
            .get(&descriptor.digest)
            .map(PathBuf::as_path)
            .ok_or_else(|| anyhow::anyhow!("MissingBlob"))
    }
    pub fn bytes(&self, descriptor: &Descriptor) -> Result<Vec<u8>> {
        ensure!(
            descriptor.size <= 2 * 1024 * 1024,
            "metadata object too large"
        );
        Ok(std::fs::read(self.path(descriptor)?)?)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImageIndex {
    schema_version: u32,
    #[serde(default)]
    media_type: Option<String>,
    manifests: Vec<Descriptor>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImageManifest {
    schema_version: u32,
    media_type: String,
    config: Descriptor,
    layers: Vec<Descriptor>,
    #[serde(default)]
    subject: Option<Descriptor>,
}
#[derive(Deserialize)]
struct ImageConfig {
    architecture: String,
    os: String,
    rootfs: RootFs,
}
#[derive(Deserialize)]
struct RootFs {
    r#type: String,
    diff_ids: Vec<String>,
}

pub fn verify_archive(
    path: &Path,
    expected: &Descriptor,
    platform: &Platform,
    limits: Limits,
) -> Result<VerifiedImage> {
    verify_layout(path, Some(expected), platform, limits)
}
/// Discover the single OCI export root, then verify every original graph byte.
pub fn verify_export_archive(
    path: &Path,
    platform: &Platform,
    limits: Limits,
) -> Result<VerifiedImage> {
    verify_layout(path, None, platform, limits)
}
fn verify_layout(
    path: &Path,
    expected: Option<&Descriptor>,
    platform: &Platform,
    limits: Limits,
) -> Result<VerifiedImage> {
    ensure!(platform == &Platform::linux_amd64(), "UnsupportedPlatform");
    if let Some(expected) = expected {
        ensure!(valid_digest(&expected.digest), "InvalidRootDigest");
        ensure!(
            expected.platform.as_ref().is_none_or(|p| p == platform),
            "WrongPlatform"
        );
    }
    let archive = File::open(path)?;
    ensure!(
        archive.metadata()?.len() <= limits.archive_bytes,
        "ArchiveTooLarge"
    );
    let spool = tempfile::tempdir()?;
    let mut paths = BTreeMap::new();
    let mut seen = BTreeSet::new();
    let mut layout = None;
    let mut index = None;
    let mut total = 0_u64;
    for (number, entry) in tar::Archive::new(archive).entries()?.enumerate() {
        ensure!(number < limits.members, "TooManyArchiveMembers");
        let mut entry = entry.context("TruncatedArchive")?;
        let path = entry.path()?.into_owned();
        ensure!(
            path.components().all(|c| matches!(c, Component::Normal(_))),
            "UnsafeArchivePath"
        );
        let path = path.to_str().context("NonUtf8ArchivePath")?;
        ensure!(seen.insert(path.to_owned()), "DuplicateArchivePath");
        if entry.header().entry_type().is_dir() {
            continue;
        }
        ensure!(
            entry.header().entry_type().is_file(),
            "UnsupportedArchiveMember"
        );
        let size = entry.size();
        total = total.checked_add(size).context("ArchiveSizeOverflow")?;
        ensure!(
            total <= limits.archive_bytes && size <= limits.blob_bytes,
            "ArchiveTooLarge"
        );
        if path == "oci-layout" || path == "index.json" {
            ensure!(size <= 2 * 1024 * 1024, "MetadataTooLarge");
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes)?;
            ensure!(bytes.len() as u64 == size, "TruncatedArchive");
            if path == "oci-layout" {
                layout = Some(bytes);
            } else {
                index = Some(bytes);
            }
        } else {
            let hash = path
                .strip_prefix("blobs/sha256/")
                .context("UnexpectedArchiveMember")?;
            let digest = format!("sha256:{hash}");
            ensure!(valid_digest(&digest), "InvalidBlobPath");
            let destination = spool.path().join(hash);
            let mut file = File::create(&destination)?;
            let (actual, count) = copy_hash(&mut entry, &mut file, limits.blob_bytes)?;
            ensure!(
                actual == digest && count == size,
                "BlobDigestOrSizeMismatch"
            );
            paths.insert(digest, destination);
        }
    }
    let layout: serde_json::Value = serde_json::from_slice(&layout.context("MissingOciLayout")?)?;
    ensure!(
        layout.get("imageLayoutVersion").and_then(|v| v.as_str()) == Some("1.0.0"),
        "UnsupportedLayoutVersion"
    );
    let index: ImageIndex = serde_json::from_slice(&index.context("MissingIndex")?)?;
    ensure!(
        index.schema_version == 2
            && index.manifests.len() == 1
            && index.media_type.as_deref().is_none_or(|m| m == INDEX),
        "OnlySinglePlatformSupported"
    );
    let selected = &index.manifests[0];
    let expected = expected.unwrap_or(selected);
    ensure!(
        selected.digest == expected.digest
            && selected.size == expected.size
            && selected.media_type == expected.media_type,
        "RootDescriptorMismatch"
    );
    let mut verified = VerifiedImage {
        _spool: spool,
        root: expected.clone(),
        manifests: vec![],
        blobs: vec![],
        paths,
    };
    check_descriptor(&verified, expected, limits)?;
    if let Some(selected) = &selected.platform {
        ensure!(selected == platform, "WrongPlatform");
    }
    let mut ancestors = Vec::new();
    let mut manifest = expected.clone();
    while manifest.media_type == INDEX {
        ensure!(ancestors.len() < 8, "IndexDepthExceeded");
        let index: ImageIndex = serde_json::from_slice(&verified.bytes(&manifest)?)?;
        ensure!(
            index.schema_version == 2
                && index.manifests.len() == 1
                && index.media_type.as_deref().is_none_or(|m| m == INDEX),
            "OnlySinglePlatformSupported"
        );
        let child = index.manifests.into_iter().next().unwrap();
        if let Some(selected) = &child.platform {
            ensure!(selected == platform, "WrongPlatform");
        }
        check_descriptor(&verified, &child, limits)?;
        ancestors.push(manifest);
        manifest = child;
    }
    ensure!(
        manifest.media_type == MANIFEST,
        "UnsupportedManifestMediaType"
    );
    let content: ImageManifest = serde_json::from_slice(&verified.bytes(&manifest)?)?;
    ensure!(content.subject.is_none(), "UnsupportedSubjectGraph");
    ensure!(
        content.schema_version == 2
            && content.media_type == MANIFEST
            && content.config.media_type == CONFIG,
        "InvalidImageManifest"
    );
    check_descriptor(&verified, &content.config, limits)?;
    let config: ImageConfig = serde_json::from_slice(&verified.bytes(&content.config)?)?;
    ensure!(
        config.os == platform.os && config.architecture == platform.architecture,
        "WrongPlatform"
    );
    ensure!(
        config.rootfs.r#type == "layers" && config.rootfs.diff_ids.len() == content.layers.len(),
        "InvalidDiffIds"
    );
    verified.blobs.push(content.config);
    let mut total_expanded = 0_u64;
    for (layer, expected_diffid) in content.layers.into_iter().zip(config.rootfs.diff_ids) {
        check_descriptor(&verified, &layer, limits)?;
        ensure!(valid_digest(&expected_diffid), "InvalidDiffId");
        let input = File::open(verified.path(&layer)?)?;
        let mut decoded: Box<dyn Read> = match layer.media_type.as_str() {
            LAYER => Box::new(input),
            GZIP_LAYER => Box::new(flate2::read::MultiGzDecoder::new(input)),
            _ => bail!("UnsupportedLayerMediaType"),
        };
        let (diffid, expanded_bytes) = copy_hash(
            &mut decoded,
            &mut std::io::sink(),
            limits.expanded_layer_bytes - total_expanded,
        )?;
        total_expanded += expanded_bytes;
        ensure!(diffid == expected_diffid, "DiffIdMismatch");
        // A digest-correct but malformed tar must not become a usable image.
        // Validate without extracting layer-controlled names into our filesystem.
        let input = File::open(verified.path(&layer)?)?;
        let decoded: Box<dyn Read> = match layer.media_type.as_str() {
            LAYER => Box::new(input),
            GZIP_LAYER => Box::new(flate2::read::MultiGzDecoder::new(input)),
            _ => unreachable!(),
        };
        let mut expanded = 0_u64;
        for entry in tar::Archive::new(decoded).entries()? {
            let mut entry = entry.context("InvalidLayerArchive")?;
            expanded = expanded
                .checked_add(entry.size())
                .context("LayerSizeOverflow")?;
            ensure!(
                expanded <= limits.expanded_layer_bytes,
                "ExpandedLayerTooLarge"
            );
            let declared = entry.size();
            ensure!(
                std::io::copy(&mut entry, &mut std::io::sink())? == declared,
                "TruncatedLayerArchive"
            );
        }
        verified.blobs.push(layer);
    }
    verified.manifests.push(manifest);
    verified.manifests.extend(ancestors.into_iter().rev());
    Ok(verified)
}
fn check_descriptor(image: &VerifiedImage, descriptor: &Descriptor, limits: Limits) -> Result<()> {
    ensure!(
        valid_digest(&descriptor.digest) && descriptor.size <= limits.blob_bytes,
        "InvalidDescriptor"
    );
    let file = File::open(image.path(descriptor)?)?;
    ensure!(
        file.metadata()?.len() == descriptor.size,
        "DescriptorSizeMismatch"
    );
    Ok(())
}
pub fn copy_hash(
    reader: &mut dyn Read,
    writer: &mut dyn Write,
    limit: u64,
) -> Result<(String, u64)> {
    let mut buffer = [0; 65536];
    let mut hash = Sha256::new();
    let mut size = 0_u64;
    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        size = size.checked_add(n as u64).context("SizeOverflow")?;
        ensure!(size <= limit, "ObjectTooLarge");
        hash.update(&buffer[..n]);
        writer.write_all(&buffer[..n])?;
    }
    Ok((format!("sha256:{}", hex::encode(hash.finalize())), size))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(os: &str, wrong_diffid: bool) -> (Descriptor, Vec<(String, Vec<u8>)>) {
        let mut layer = Vec::new();
        {
            let mut tar = tar::Builder::new(&mut layer);
            let mut header = tar::Header::new_gnu();
            header.set_size(7);
            header.set_mode(0o444);
            header.set_cksum();
            tar.append_data(&mut header, "fixture", &b"fixture"[..])
                .unwrap();
            tar.finish().unwrap();
        }
        let config=serde_json::to_vec(&serde_json::json!({"os":os,"architecture":"amd64","rootfs":{"type":"layers","diff_ids":[if wrong_diffid {digest(b"other")} else {digest(&layer)}]}})).unwrap();
        let config_d = descriptor(&config, CONFIG);
        let layer_d = descriptor(&layer, LAYER);
        let manifest=serde_json::to_vec(&serde_json::json!({"schemaVersion":2,"mediaType":MANIFEST,"config":config_d,"layers":[layer_d]})).unwrap();
        let root = descriptor(&manifest, MANIFEST);
        let index =
            serde_json::to_vec(&serde_json::json!({"schemaVersion":2,"manifests":[root]})).unwrap();
        let mut entries = vec![
            (
                "oci-layout".into(),
                br#"{"imageLayoutVersion":"1.0.0"}"#.to_vec(),
            ),
            ("index.json".into(), index),
        ];
        for bytes in [config, layer, manifest] {
            entries.push((format!("blobs/sha256/{}", &digest(&bytes)[7..]), bytes));
        }
        (root, entries)
    }
    fn write_archive(entries: &[(String, Vec<u8>)]) -> tempfile::NamedTempFile {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut archive = tar::Builder::new(File::create(file.path()).unwrap());
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
        file
    }
    #[test]
    fn complete_graph_validation_rejects_corruption_missing_size_platform_and_diffids() {
        let (root, entries) = fixture("linux", false);
        let valid = write_archive(&entries);
        assert_eq!(
            verify_export_archive(valid.path(), &Platform::linux_amd64(), Limits::default())
                .unwrap()
                .root,
            root
        );
        let mut annotated = entries.clone();
        let mut index: serde_json::Value = serde_json::from_slice(&annotated[1].1).unwrap();
        index["manifests"][0]["annotations"] =
            serde_json::json!({"org.opencontainers.image.ref.name":"export"});
        annotated[1].1 = serde_json::to_vec(&index).unwrap();
        assert!(
            verify_export_archive(
                write_archive(&annotated).path(),
                &Platform::linux_amd64(),
                Limits::default()
            )
            .is_ok()
        );
        index["manifests"][0]["urls"] = serde_json::json!(["https://untrusted/blob"]);
        annotated[1].1 = serde_json::to_vec(&index).unwrap();
        assert!(
            verify_export_archive(
                write_archive(&annotated).path(),
                &Platform::linux_amd64(),
                Limits::default()
            )
            .is_err()
        );
        assert!(
            verify_archive(
                valid.path(),
                &root,
                &Platform::linux_amd64(),
                Limits::default()
            )
            .is_ok()
        );
        let mut missing = entries.clone();
        missing.remove(2);
        assert!(
            verify_archive(
                write_archive(&missing).path(),
                &root,
                &Platform::linux_amd64(),
                Limits::default()
            )
            .is_err()
        );
        let mut corrupt = entries.clone();
        corrupt[2].1[0] ^= 1;
        assert!(
            verify_archive(
                write_archive(&corrupt).path(),
                &root,
                &Platform::linux_amd64(),
                Limits::default()
            )
            .is_err()
        );
        let mut wrong_size = root.clone();
        wrong_size.size += 1;
        assert!(
            verify_archive(
                valid.path(),
                &wrong_size,
                &Platform::linux_amd64(),
                Limits::default()
            )
            .is_err()
        );
        for (os, diffid) in [("windows", false), ("linux", true)] {
            let (root, entries) = fixture(os, diffid);
            assert!(
                verify_archive(
                    write_archive(&entries).path(),
                    &root,
                    &Platform::linux_amd64(),
                    Limits::default()
                )
                .is_err()
            );
        }
        let mut duplicate = entries.clone();
        duplicate.push(entries[2].clone());
        assert!(
            verify_archive(
                write_archive(&duplicate).path(),
                &root,
                &Platform::linux_amd64(),
                Limits::default()
            )
            .is_err()
        );
        assert!(
            verify_archive(
                valid.path(),
                &root,
                &Platform::linux_amd64(),
                Limits {
                    archive_bytes: 512,
                    ..Limits::default()
                }
            )
            .is_err()
        );
    }
    #[test]
    fn sha256_is_strict() {
        assert!(valid_digest(&digest(b"hello")));
        assert!(!valid_digest(&format!("sha256:{}", "A".repeat(64))));
        assert!(!valid_digest("sha256:../file"));
    }
    #[test]
    fn compressed_layers_and_nested_indexes_keep_original_roots() {
        let (_, entries) = fixture("linux", false);
        let config = entries[2].1.clone();
        let uncompressed = entries[3].1.clone();
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&uncompressed).unwrap();
        let compressed = encoder.finish().unwrap();
        let manifest = serde_json::to_vec(&serde_json::json!({"schemaVersion":2,"mediaType":MANIFEST,"config":descriptor(&config,CONFIG),"layers":[descriptor(&compressed,GZIP_LAYER)]})).unwrap();
        let mut child = descriptor(&manifest, MANIFEST);
        child.platform = Some(Platform::linux_amd64());
        let inner = serde_json::to_vec(
            &serde_json::json!({"schemaVersion":2,"mediaType":INDEX,"manifests":[child]}),
        )
        .unwrap();
        let outer = serde_json::to_vec(&serde_json::json!({"schemaVersion":2,"mediaType":INDEX,"manifests":[descriptor(&inner,INDEX)]})).unwrap();
        let root = descriptor(&outer, INDEX);
        let mut entries = vec![
            ("oci-layout".into(), entries[0].1.clone()),
            (
                "index.json".into(),
                serde_json::to_vec(&serde_json::json!({"schemaVersion":2,"manifests":[root]}))
                    .unwrap(),
            ),
        ];
        for blob in [&config, &compressed, &manifest, &inner, &outer] {
            entries.push((format!("blobs/sha256/{}", &digest(blob)[7..]), blob.clone()));
        }
        let archive = write_archive(&entries);
        let image = verify_archive(
            archive.path(),
            &root,
            &Platform::linux_amd64(),
            Limits::default(),
        )
        .unwrap();
        assert_eq!(image.root, root);
        assert_eq!(image.manifests.len(), 3);
        assert_eq!(image.bytes(&root).unwrap(), outer);
        assert_eq!(image.blobs[1].digest, digest(&compressed));
        assert!(
            verify_archive(
                archive.path(),
                &root,
                &Platform::linux_amd64(),
                Limits {
                    expanded_layer_bytes: uncompressed.len() as u64 - 1,
                    ..Limits::default()
                }
            )
            .is_err()
        );
    }
    #[test]
    fn rejects_truncated_archive_and_bad_root() {
        let f = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(f.path(), b"not a tar").unwrap();
        assert!(
            verify_archive(
                f.path(),
                &descriptor(b"x", MANIFEST),
                &Platform::linux_amd64(),
                Limits::default()
            )
            .is_err()
        );
    }
}

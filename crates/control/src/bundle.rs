//! Offline inventory integrity. Trust its digest through an independent delivery channel.
use anyhow::{Context, Result, ensure};
use kimage_core::oci::{Descriptor, Platform};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileEntry {
    pub file: String,
    pub size: u64,
    pub digest: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Component {
    pub version: String,
    pub executable: FileEntry,
    pub archive: FileEntry,
    pub root: Descriptor,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Inventory {
    pub schema: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_schema_revision: Option<u32>,
    pub platform: Platform,
    pub launcher: FileEntry,
    pub components: BTreeMap<String, Component>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub installers: BTreeMap<String, FileEntry>,
}
pub struct Bundle {
    pub root: PathBuf,
    pub inventory: Inventory,
    pub digest: String,
}
impl Bundle {
    pub fn open(root: &Path, trusted_digest: &str) -> Result<Self> {
        ensure!(
            kimage_core::oci::valid_digest(trusted_digest),
            "TrustedInventoryDigestRequired"
        );
        let path = root.join("inventory.json");
        ensure!(
            std::fs::symlink_metadata(&path)?.is_file(),
            "InvalidInventoryFile"
        );
        let metadata = std::fs::metadata(&path)?;
        ensure!(metadata.len() <= 256 * 1024, "InventoryTooLarge");
        let bytes = std::fs::read(path)?;
        ensure!(
            kimage_core::oci::digest(&bytes) == trusted_digest,
            "InventoryDigestMismatch"
        );
        let inventory: Inventory = serde_json::from_slice(&bytes)?;
        ensure!(
            inventory.schema == 1 && inventory.platform == Platform::linux_amd64(),
            "UnsupportedBundle"
        );
        let bundle = Self {
            root: root.to_owned(),
            inventory,
            digest: trusted_digest.into(),
        };
        bundle.check_file(&bundle.inventory.launcher, 512 * 1024)?;
        bundle.check_static(&bundle.inventory.launcher)?;
        for name in [
            "registry",
            "gateway",
            "proxy",
            "operator",
            "admission",
            "input",
        ] {
            ensure!(
                bundle.inventory.components.contains_key(name),
                "MissingBundleComponent: {name}"
            );
        }
        ensure!(
            bundle.inventory.components.len() <= 16,
            "TooManyBundleComponents"
        );
        ensure!(
            matches!(
                bundle.inventory.components["registry"].version.as_str(),
                "3.1.1" | "3.1.2"
            ),
            "UnsupportedRegistryVersion"
        );
        for component in bundle.inventory.components.values() {
            bundle.check_file(&component.executable, 128 * 1024 * 1024)?;
            bundle.check_static(&component.executable)?;
            bundle.check_file(&component.archive, 1024 * 1024 * 1024)?;
            kimage_core::oci::verify_archive(
                &bundle.path(&component.archive)?,
                &component.root,
                &bundle.inventory.platform,
                kimage_core::oci::Limits::default(),
            )?;
        }
        ensure!(
            bundle.inventory.installers.len() <= 2,
            "TooManyNativeInstallers"
        );
        for (platform, entry) in &bundle.inventory.installers {
            bundle.check_file(entry, 128 * 1024 * 1024)?;
            match platform.as_str() {
                "linux-amd64" => bundle.check_static(entry)?,
                "darwin-arm64" => {
                    let mut header = [0_u8; 16];
                    std::fs::File::open(bundle.path(entry)?)?.read_exact(&mut header)?;
                    ensure!(
                        header[..8] == [207, 250, 237, 254, 12, 0, 0, 1]
                            && header[12..] == [2, 0, 0, 0],
                        "NotNativeDarwinArm64Installer"
                    );
                }
                _ => anyhow::bail!("UnsupportedInstallerPlatform"),
            }
        }
        Ok(bundle)
    }
    pub fn path(&self, entry: &FileEntry) -> Result<PathBuf> {
        ensure!(
            !entry.file.is_empty()
                && entry.file.len() <= 128
                && !entry.file.starts_with('.')
                && entry
                    .file
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_')),
            "UnsafeBundlePath"
        );
        let path = self.root.join(&entry.file);
        ensure!(
            std::fs::symlink_metadata(&path)?.is_file(),
            "NonRegularBundleFile"
        );
        Ok(path)
    }
    fn check_file(&self, entry: &FileEntry, limit: u64) -> Result<()> {
        let file = std::fs::File::open(self.path(entry)?)?;
        ensure!(
            file.metadata()?.len() == entry.size && entry.size <= limit,
            "BundleFileSizeMismatch"
        );
        let (digest, size) = kimage_core::oci::copy_hash(
            &mut std::io::BufReader::new(file),
            &mut std::io::sink(),
            limit,
        )?;
        ensure!(
            digest == entry.digest && size == entry.size,
            "BundleFileDigestMismatch"
        );
        Ok(())
    }
    fn check_static(&self, entry: &FileEntry) -> Result<()> {
        let mut file = std::fs::File::open(self.path(entry)?)?;
        validate_static(&mut file)
    }
    pub fn chunks(&self, name: &str) -> Result<(Vec<Vec<u8>>, String)> {
        let component = self
            .inventory
            .components
            .get(name)
            .context("UnknownBundleComponent")?;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut input = std::fs::File::open(self.path(&component.executable)?)?;
        let mut header = [0; 20];
        input.read_exact(&mut header)?;
        ensure!(
            &header[0..4] == b"\x7fELF"
                && header[4] == 2
                && header[5] == 1
                && header[18..20] == [62, 0],
            "NotLinuxAmd64Executable"
        );
        use std::io::{Seek, Write};
        input.rewind()?;
        let (actual, size) =
            kimage_core::oci::copy_hash(&mut input, &mut encoder, 128 * 1024 * 1024)?;
        ensure!(
            actual == component.executable.digest && size == component.executable.size,
            "BundleChangedDuringCapture"
        );
        encoder.flush()?;
        let bytes = encoder.finish()?;
        ensure!(
            bytes.len() <= 128 * 1024 * 1024,
            "CompressedPayloadTooLarge"
        );
        let digest = kimage_core::oci::digest(&bytes)[7..].to_owned();
        Ok((
            bytes.chunks(512 * 1024).map(<[u8]>::to_vec).collect(),
            digest,
        ))
    }
}
fn validate_static(file: &mut std::fs::File) -> Result<()> {
    use std::io::{Seek, SeekFrom};
    let mut header = [0_u8; 64];
    file.read_exact(&mut header)?;
    ensure!(
        &header[..6] == b"\x7fELF\x02\x01" && header[18..20] == [62, 0],
        "NotLinuxAmd64Executable"
    );
    let offset = u64::from_le_bytes(header[32..40].try_into().unwrap());
    let stride = u16::from_le_bytes(header[54..56].try_into().unwrap()) as u64;
    let count = u16::from_le_bytes(header[56..58].try_into().unwrap()) as u64;
    ensure!(
        (56..=256).contains(&stride)
            && (1..=256).contains(&count)
            && offset
                .checked_add(stride * count)
                .is_some_and(|end| end <= file.metadata().map(|m| m.len()).unwrap_or(0)),
        "InvalidExecutableHeaders"
    );
    for i in 0..count {
        file.seek(SeekFrom::Start(offset + i * stride))?;
        let mut kind = [0; 4];
        file.read_exact(&mut kind)?;
        ensure!(
            u32::from_le_bytes(kind) != 3,
            "DynamicExecutableUnsupported"
        );
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_or_dynamic_launchers_fail_before_cluster_mutation() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut bytes = vec![0_u8; 120];
        bytes[..6].copy_from_slice(b"\x7fELF\x02\x01");
        bytes[18..20].copy_from_slice(&62_u16.to_le_bytes());
        bytes[32..40].copy_from_slice(&64_u64.to_le_bytes());
        bytes[54..56].copy_from_slice(&56_u16.to_le_bytes());
        bytes[56..58].copy_from_slice(&1_u16.to_le_bytes());
        std::fs::write(file.path(), &bytes).unwrap();
        assert!(validate_static(&mut std::fs::File::open(file.path()).unwrap()).is_ok());
        bytes[64..68].copy_from_slice(&3_u32.to_le_bytes());
        std::fs::write(file.path(), &bytes).unwrap();
        assert!(validate_static(&mut std::fs::File::open(file.path()).unwrap()).is_err());
        bytes.truncate(70);
        std::fs::write(file.path(), bytes).unwrap();
        assert!(validate_static(&mut std::fs::File::open(file.path()).unwrap()).is_err());
    }
}

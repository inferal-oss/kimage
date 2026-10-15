//! Official release delivery. GitHub's HTTPS API authenticates asset digests;
//! inventory, executable, and OCI verification still precede cluster access.
use crate::bundle::{Bundle, FileEntry};
use anyhow::{Context, Result, ensure};
use futures::StreamExt;
use kimage_core::oci::Platform;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions, TryLockError},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::Duration,
};

const REPOSITORY: &str = "inferal-oss/kimage";
const MAX_METADATA: u64 = 2 * 1024 * 1024;
const MAX_ARCHIVE: u64 = 1024 * 1024 * 1024;
const MAX_EXPANDED: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    assets: Vec<Asset>,
}
#[derive(Debug, Clone, Deserialize)]
struct Asset {
    name: String,
    size: u64,
    digest: Option<String>,
    browser_download_url: String,
    state: String,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Receipt {
    #[serde(default = "Platform::linux_amd64")]
    platform: Platform,
    schema: u32,
    repository: String,
    tag: String,
    archive: FileEntry,
    inventory: FileEntry,
}

pub fn release_tag(value: &str) -> Result<String> {
    ensure!(value.len() <= 64, "ReleaseVersionTooLong");
    let version = value.strip_prefix('v').unwrap_or(value);
    let parsed = kimage_core::selector::exact(version).context("ExactReleaseVersionRequired")?;
    ensure!(
        parsed.to_string() == version,
        "CanonicalReleaseVersionRequired"
    );
    Ok(format!("v{version}"))
}

pub fn default_cache() -> Result<PathBuf> {
    if let Some(cache) = std::env::var_os("XDG_CACHE_HOME") {
        ensure!(!cache.is_empty(), "EmptyCacheDirectory");
        return Ok(PathBuf::from(cache).join("kimage/bundles"));
    }
    let home = PathBuf::from(std::env::var_os("HOME").context("BundleCacheRequiredWithoutHome")?);
    Ok(home.join(if cfg!(target_os = "macos") {
        "Library/Caches/kimage/bundles"
    } else {
        ".cache/kimage/bundles"
    }))
}

/// Local bundles retain their independent digest requirement. Remote selection
/// is exact, never "latest", and a cached release never needs a network request.
pub async fn resolve(
    local: Option<&Path>,
    trusted: Option<&str>,
    release: Option<&str>,
    cache: Option<&Path>,
    offline: bool,
) -> Result<Bundle> {
    resolve_for_platform(local, trusted, release, cache, offline, &Platform::native()).await
}

pub async fn resolve_for_platform(
    local: Option<&Path>,
    trusted: Option<&str>,
    release: Option<&str>,
    cache: Option<&Path>,
    offline: bool,
    platform: &Platform,
) -> Result<Bundle> {
    ensure!(platform.supported(), "UnsupportedBundlePlatform");
    if let Some(local) = local {
        ensure!(release.is_none(), "LocalBundleAndReleaseConflict");
        let bundle = Bundle::open(
            local,
            trusted.context("LocalBundleRequiresTrustedInventory")?,
        )?;
        ensure!(
            &bundle.inventory.platform == platform,
            "BundleNodePlatformMismatch"
        );
        return Ok(bundle);
    }
    let tag = release_tag(release.unwrap_or(env!("CARGO_PKG_VERSION")))?;
    if let Some(digest) = trusted {
        ensure!(
            kimage_core::oci::valid_digest(digest),
            "TrustedInventoryDigestRequired"
        );
    }
    let cache = cache
        .map(Path::to_path_buf)
        .map(Ok)
        .unwrap_or_else(default_cache)?;
    let directory = cache.join(format!("{tag}-linux-{}", platform.architecture));
    if let Some(bundle) = cached(&directory, &tag, trusted)? {
        ensure!(
            &bundle.inventory.platform == platform,
            "CachedBundlePlatformMismatch"
        );
        return Ok(bundle);
    }
    ensure!(
        !offline,
        "ReleaseNotCached: fetch {tag} online first, or supply --bundle and --trusted-inventory"
    );
    private_directory(&cache)?;
    let lock_path = cache.join(format!(".{tag}-linux-{}.lock", platform.architecture));
    if lock_path.exists() {
        ensure!(
            fs::symlink_metadata(&lock_path)?.is_file(),
            "InvalidBundleCacheLock"
        );
    }
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(lock_path)?;
    tokio::time::timeout(Duration::from_secs(720), async {
        loop {
            match lock.try_lock() {
                Ok(()) => break Ok::<_, anyhow::Error>(()),
                Err(TryLockError::WouldBlock) => {
                    tokio::time::sleep(Duration::from_millis(250)).await
                }
                Err(TryLockError::Error(error)) => return Err(error.into()),
            }
        }
    })
    .await
    .context("BundleCacheLockTimeout")??;
    if let Some(bundle) = cached(&directory, &tag, trusted)? {
        ensure!(
            &bundle.inventory.platform == platform,
            "CachedBundlePlatformMismatch"
        );
        return Ok(bundle);
    }
    let client = Client::official(platform.clone())?;
    client.fetch(&cache, &directory, &tag, trusted).await
}

fn private_directory(path: &Path) -> Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_dir() && metadata.permissions().mode() & 0o022 == 0,
        "BundleCacheMustNotBeSymlinkOrWritableByOthers"
    );
    Ok(())
}

fn names_for(tag: &str, platform: &Platform) -> (String, String) {
    let base = format!("kimage-bundle-{tag}-linux-{}", platform.architecture);
    (format!("{base}.tar.gz"), format!("{base}.inventory.json"))
}

fn checked_bundle(
    directory: &Path,
    receipt: &Receipt,
    tag: &str,
    trusted: Option<&str>,
) -> Result<Bundle> {
    let (archive_name, inventory_name) = names_for(tag, &receipt.platform);
    ensure!(
        receipt.schema == 1
            && receipt.platform.supported()
            && receipt.repository == REPOSITORY
            && receipt.tag == tag
            && receipt.archive.file == archive_name
            && receipt.inventory.file == inventory_name,
        "InvalidReleaseReceipt"
    );
    ensure!(
        kimage_core::oci::valid_digest(&receipt.archive.digest)
            && kimage_core::oci::valid_digest(&receipt.inventory.digest)
            && receipt.archive.size > 0
            && receipt.archive.size <= MAX_ARCHIVE
            && receipt.inventory.size > 0
            && receipt.inventory.size <= 256 * 1024,
        "InvalidReleaseReceiptBounds"
    );
    if let Some(expected) = trusted {
        ensure!(
            receipt.inventory.digest == expected,
            "ReleaseInventoryPinMismatch"
        );
    }
    let root = directory.join("bundle");
    ensure!(
        fs::symlink_metadata(&root)?.is_dir(),
        "InvalidCachedBundleDirectory"
    );
    let bundle = Bundle::open(&root, &receipt.inventory.digest)?;
    ensure!(
        bundle.inventory.platform == receipt.platform,
        "ReleaseBundlePlatformMismatch"
    );
    ensure!(
        fs::metadata(root.join("inventory.json"))?.len() == receipt.inventory.size,
        "ReleaseInventorySizeMismatch"
    );
    let version = tag.strip_prefix('v').context("InvalidReleaseTag")?;
    for role in ["operator", "admission", "input", "exporter"] {
        ensure!(
            bundle
                .inventory
                .components
                .get(role)
                .is_some_and(|component| component.version == version),
            "ReleaseComponentVersionMismatch: {role}"
        );
    }
    ensure!(
        bundle.inventory.components.contains_key("builder")
            && bundle
                .inventory
                .installers
                .contains_key(&format!("linux-{}", receipt.platform.architecture))
            && bundle
                .inventory
                .installers
                .keys()
                .any(|key| key.starts_with("darwin-")),
        "IncompleteReleaseBundle"
    );
    let mut expected = BTreeSet::from([
        "inventory.json".to_owned(),
        bundle.inventory.launcher.file.clone(),
    ]);
    for component in bundle.inventory.components.values() {
        expected.insert(component.executable.file.clone());
        expected.insert(component.archive.file.clone());
    }
    expected.extend(
        bundle
            .inventory
            .installers
            .values()
            .map(|entry| entry.file.clone()),
    );
    let actual: BTreeSet<_> = fs::read_dir(&root)?
        .map(|entry| {
            let entry = entry?;
            ensure!(entry.file_type()?.is_file(), "InvalidReleaseFile");
            Ok(entry
                .file_name()
                .to_str()
                .context("InvalidReleaseFilename")?
                .to_owned())
        })
        .collect::<Result<_>>()?;
    ensure!(actual == expected, "UnexpectedReleaseFiles");
    Ok(bundle)
}

fn cached(directory: &Path, tag: &str, trusted: Option<&str>) -> Result<Option<Bundle>> {
    match fs::symlink_metadata(directory) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        metadata => ensure!(metadata?.is_dir(), "InvalidBundleCacheDirectory"),
    }
    let parent = fs::symlink_metadata(directory.parent().context("InvalidCacheRoot")?)?;
    ensure!(
        parent.is_dir() && parent.permissions().mode() & 0o022 == 0,
        "UnsafeBundleCacheRoot"
    );
    let path = directory.join("github-release.json");
    ensure!(
        fs::symlink_metadata(&path)?.is_file() && fs::metadata(&path)?.len() <= 64 * 1024,
        "InvalidReleaseReceiptFile"
    );
    let receipt: Receipt = serde_json::from_slice(&fs::read(path)?)?;
    checked_bundle(directory, &receipt, tag, trusted).context("CachedBundleInvalid: preserve independently trusted bundles; remove this cache entry explicitly before refetching").map(Some)
}

struct Client {
    platform: Platform,
    http: reqwest::Client,
    api: reqwest::Url,
    assets: reqwest::Url,
    api_token: Option<reqwest::header::HeaderValue>,
}
fn trusted_redirect(url: &reqwest::Url) -> bool {
    url.scheme() == "https"
        && url.port_or_known_default() == Some(443)
        && url.username().is_empty()
        && url.password().is_none()
        && matches!(
            url.host_str(),
            Some(
                "github.com"
                    | "api.github.com"
                    | "release-assets.githubusercontent.com"
                    | "objects.githubusercontent.com"
            )
        )
}
impl Client {
    fn official(platform: Platform) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("kimage-install/", env!("CARGO_PKG_VERSION")))
            .https_only(true)
            .connect_timeout(Duration::from_secs(20))
            .read_timeout(Duration::from_secs(60))
            .timeout(Duration::from_secs(600))
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                let url = attempt.url();
                if attempt
                    .previous()
                    .first()
                    .is_some_and(|origin| origin.host_str() == Some("api.github.com"))
                    || attempt.previous().len() >= 5
                    || !trusted_redirect(url)
                {
                    attempt.error("UntrustedReleaseRedirect")
                } else {
                    attempt.follow()
                }
            }))
            .build()?;
        let api_token = std::env::var("KIMAGE_GITHUB_TOKEN")
            .ok()
            .map(|token| -> Result<_> {
                ensure!(
                    !token.is_empty() && token.len() <= 4096,
                    "InvalidGitHubApiToken"
                );
                let mut header = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))
                    .context("InvalidGitHubApiToken")?;
                header.set_sensitive(true);
                Ok(header)
            })
            .transpose()?;
        Ok(Self {
            platform,
            http,
            api: "https://api.github.com/repos/inferal-oss/kimage/releases/tags/".parse()?,
            assets: "https://github.com/inferal-oss/kimage/releases/download/".parse()?,
            api_token,
        })
    }

    async fn metadata(&self, tag: &str) -> Result<Receipt> {
        let mut request = self
            .http
            .get(self.api.join(tag)?)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2026-03-10");
        if let Some(token) = &self.api_token {
            request = request.header(reqwest::header::AUTHORIZATION, token.clone());
        }
        let mut response = request
            .send()
            .await
            .map_err(reqwest::Error::without_url)
            .context("ReleaseMetadataRequestFailed")?;
        ensure!(
            response.status() == reqwest::StatusCode::OK,
            "ReleaseUnavailable: {tag}, HTTP {}; choose a published release or use an independently trusted local bundle",
            response.status()
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(reqwest::Error::without_url)?
        {
            ensure!(
                bytes.len() as u64 + chunk.len() as u64 <= MAX_METADATA,
                "ReleaseMetadataTooLarge"
            );
            bytes.extend_from_slice(&chunk);
        }
        let release: Release = serde_json::from_slice(&bytes)?;
        ensure!(
            release.tag_name == tag && !release.draft && release.assets.len() <= 64,
            "InvalidOfficialRelease"
        );
        let (archive, inventory) = names_for(tag, &self.platform);
        let select = |name: String, limit| -> Result<FileEntry> {
            let matches: Vec<_> = release
                .assets
                .iter()
                .filter(|asset| asset.name == name)
                .collect();
            ensure!(matches.len() == 1, "MissingOrDuplicateReleaseAsset: {name}");
            let asset = matches[0];
            ensure!(
                asset.state == "uploaded" && asset.size > 0 && asset.size <= limit,
                "InvalidReleaseAssetBounds"
            );
            ensure!(
                asset.browser_download_url == self.assets.join(&format!("{tag}/{name}"))?.as_str(),
                "UntrustedReleaseAssetUrl"
            );
            let digest = asset.digest.as_ref().context("GitHubAssetDigestRequired")?;
            ensure!(
                kimage_core::oci::valid_digest(digest),
                "InvalidGitHubAssetDigest"
            );
            Ok(FileEntry {
                file: name,
                size: asset.size,
                digest: digest.clone(),
            })
        };
        Ok(Receipt {
            platform: self.platform.clone(),
            schema: 1,
            repository: REPOSITORY.into(),
            tag: tag.into(),
            archive: select(archive, MAX_ARCHIVE)?,
            inventory: select(inventory, 256 * 1024)?,
        })
    }

    async fn fetch(
        &self,
        cache: &Path,
        destination: &Path,
        tag: &str,
        trusted: Option<&str>,
    ) -> Result<Bundle> {
        let receipt = self.metadata(tag).await?;
        if let Some(expected) = trusted {
            ensure!(
                expected == receipt.inventory.digest,
                "ReleaseInventoryPinMismatch"
            );
        }
        tracing::info!(
            release = tag,
            bytes = receipt.archive.size,
            "fetching official release bundle"
        );
        let pending = tempfile::Builder::new()
            .prefix(".pending-")
            .tempdir_in(cache)?;
        let archive = pending.path().join("download.tar.gz");
        let response = self
            .http
            .get(
                self.assets
                    .join(&format!("{tag}/{}", receipt.archive.file))?,
            )
            .send()
            .await
            .map_err(reqwest::Error::without_url)
            .context("ReleaseDownloadFailed")?;
        ensure!(
            response.status() == reqwest::StatusCode::OK,
            "ReleaseDownloadHttpStatus: {}",
            response.status()
        );
        if let Some(size) = response.content_length() {
            ensure!(size == receipt.archive.size, "ReleaseDownloadSizeMismatch");
        }
        let mut stream = response.bytes_stream();
        let mut output = tokio::fs::File::create(&archive).await?;
        let mut hash = Sha256::new();
        let mut size = 0_u64;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk
                .map_err(reqwest::Error::without_url)
                .context("ReleaseDownloadInterrupted")?;
            size += chunk.len() as u64;
            ensure!(size <= receipt.archive.size, "ReleaseDownloadTooLarge");
            hash.update(&chunk);
            tokio::io::AsyncWriteExt::write_all(&mut output, &chunk).await?;
        }
        ensure!(
            size == receipt.archive.size
                && format!("sha256:{}", hex::encode(hash.finalize())) == receipt.archive.digest,
            "ReleaseDownloadDigestOrSizeMismatch"
        );
        output.sync_all().await?;
        drop(output);
        let bundle_root = pending.path().join("bundle");
        private_directory(&bundle_root)?;
        extract(&archive, &bundle_root)?;
        checked_bundle(pending.path(), &receipt, tag, trusted)?;
        let mut file = File::create(pending.path().join("github-release.json"))?;
        file.write_all(&serde_json::to_vec_pretty(&receipt)?)?;
        file.sync_all()?;
        fs::remove_file(&archive)?;
        fs::rename(pending.path(), destination)?;
        // A concurrent downloader is excluded by the outer process lock.
        checked_bundle(destination, &receipt, tag, trusted)
    }
}

fn safe_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && !value.starts_with('.')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

fn extract(archive: &Path, destination: &Path) -> Result<()> {
    let decoder = flate2::read::GzDecoder::new(std::io::BufReader::new(File::open(archive)?));
    let mut tar = tar::Archive::new(decoder);
    let mut files = BTreeSet::new();
    let mut total = 0_u64;
    for entry in tar.entries()?.raw(true) {
        let mut entry = entry?;
        let path = entry.path()?;
        let name = path.to_str().context("InvalidReleaseFilename")?.to_owned();
        ensure!(
            entry.header().entry_type().is_file() && safe_name(&name),
            "UnsafeReleaseArchiveEntry"
        );
        ensure!(
            files.insert(name.clone()) && files.len() <= 48,
            "DuplicateOrExcessiveReleaseEntries"
        );
        let size = entry.size();
        let limit = if name == "inventory.json" {
            256 * 1024
        } else {
            MAX_ARCHIVE
        };
        ensure!(size <= limit, "ReleaseEntryTooLarge");
        total = total
            .checked_add(size)
            .context("ReleaseExpandedSizeOverflow")?;
        ensure!(total <= MAX_EXPANDED, "ReleaseArchiveExpandedTooLarge");
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(destination.join(&name))?;
        ensure!(
            std::io::copy(&mut entry, &mut output)? == size,
            "TruncatedReleaseEntry"
        );
        output.sync_all()?;
        let mode = if (name.starts_with("kimage-") && !name.ends_with(".tar")) || name == "registry"
        {
            0o555
        } else {
            0o444
        };
        output.set_permissions(fs::Permissions::from_mode(mode))?;
    }
    // Read through the gzip trailer and bound tar padding after its end marker.
    let mut padding = Vec::new();
    tar.into_inner()
        .take(1024 * 1024 + 1)
        .read_to_end(&mut padding)?;
    ensure!(
        padding.len() <= 1024 * 1024 && padding.iter().all(|byte| *byte == 0),
        "UnexpectedReleaseArchiveTail"
    );
    ensure!(files.contains("inventory.json"), "MissingReleaseInventory");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn resolve(
        local: Option<&Path>,
        trusted: Option<&str>,
        release: Option<&str>,
        cache: Option<&Path>,
        offline: bool,
    ) -> Result<Bundle> {
        super::resolve_for_platform(
            local,
            trusted,
            release,
            cache,
            offline,
            &Platform::linux_amd64(),
        )
        .await
    }

    use axum::{
        Router,
        body::Body,
        http::{Response, StatusCode},
        response::IntoResponse,
    };
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};

    fn tar_bytes(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
        let mut archive = tar::Builder::new(Vec::new());
        for (name, bytes) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o555);
            header.set_cksum();
            archive
                .append_data(&mut header, name, bytes.as_slice())
                .unwrap();
        }
        archive.into_inner().unwrap()
    }
    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }
    fn fixture() -> (Vec<u8>, Vec<u8>) {
        fixture_for(&Platform::linux_amd64())
    }
    fn fixture_for(platform: &Platform) -> (Vec<u8>, Vec<u8>) {
        use kimage_core::oci::{CONFIG, LAYER, MANIFEST, digest};
        let layer = tar_bytes(&[("message".into(), b"fixture".to_vec())]);
        let config = serde_json::to_vec(&json!({"os":"linux","architecture":platform.architecture,"rootfs":{"type":"layers","diff_ids":[digest(&layer)]}})).unwrap();
        let descriptor = |bytes: &[u8], media| json!({"mediaType":media,"digest":digest(bytes),"size":bytes.len()});
        let manifest = serde_json::to_vec(&json!({"schemaVersion":2,"mediaType":MANIFEST,"config":descriptor(&config,CONFIG),"layers":[descriptor(&layer,LAYER)]})).unwrap();
        let root = descriptor(&manifest, MANIFEST);
        let mut oci = vec![
            (
                "oci-layout".into(),
                br#"{"imageLayoutVersion":"1.0.0"}"#.to_vec(),
            ),
            (
                "index.json".into(),
                serde_json::to_vec(&json!({"schemaVersion":2,"manifests":[root]})).unwrap(),
            ),
        ];
        for bytes in [layer, config, manifest] {
            oci.push((format!("blobs/sha256/{}", &digest(&bytes)[7..]), bytes));
        }
        let oci = tar_bytes(&oci);
        let mut elf = vec![0_u8; 120];
        elf[..6].copy_from_slice(b"\x7fELF\x02\x01");
        elf[18..20].copy_from_slice(
            &(if platform.architecture == "amd64" {
                62_u16
            } else {
                183_u16
            })
            .to_le_bytes(),
        );
        elf[32..40].copy_from_slice(&64_u64.to_le_bytes());
        elf[54..56].copy_from_slice(&56_u16.to_le_bytes());
        elf[56..58].copy_from_slice(&1_u16.to_le_bytes());
        let mut mach = vec![0_u8; 16];
        mach[..8].copy_from_slice(&[207, 250, 237, 254, 12, 0, 0, 1]);
        mach[12..].copy_from_slice(&[2, 0, 0, 0]);
        let entry = |name: &str, bytes: &[u8]| json!({"file":name,"size":bytes.len(),"digest":digest(bytes)});
        let mut files = vec![
            ("kimage-bootstrap".into(), elf.clone()),
            (
                format!("kimage-install-linux-{}", platform.architecture),
                elf.clone(),
            ),
            ("kimage-install-darwin-arm64".into(), mach.clone()),
        ];
        let mut components = serde_json::Map::new();
        for name in [
            "registry",
            "gateway",
            "proxy",
            "operator",
            "admission",
            "input",
            "exporter",
            "builder",
        ] {
            let executable = if name == "registry" {
                name.to_owned()
            } else {
                format!("kimage-{name}")
            };
            let archive = format!("{name}.oci.tar");
            components.insert(name.into(), json!({"version":match name {"registry"=>"3.1.2","builder"=>"0.33.0",_=>"0.2.0"},"executable":entry(&executable,&elf),"archive":entry(&archive,&oci),"root":root}));
            files.push((executable, elf.clone()));
            files.push((archive, oci.clone()));
        }
        let inventory = serde_json::to_vec(&json!({"schema":1,"apiSchemaRevision":4,"platform":platform,"launcher":entry("kimage-bootstrap",&elf),"components":components,"installers":{(format!("linux-{}", platform.architecture)):entry(&format!("kimage-install-linux-{}", platform.architecture),&elf),"darwin-arm64":entry("kimage-install-darwin-arm64",&mach)}})).unwrap();
        files.push(("inventory.json".into(), inventory.clone()));
        (gzip(&tar_bytes(&files)), inventory)
    }

    struct State {
        metadata: Value,
        archive: Vec<u8>,
        failure: Option<&'static str>,
        requests: usize,
        authenticated_metadata: usize,
    }
    struct Server {
        client: Client,
        state: Arc<Mutex<State>>,
        task: tokio::task::JoinHandle<()>,
    }
    impl Drop for Server {
        fn drop(&mut self) {
            self.task.abort();
        }
    }
    async fn server() -> Server {
        server_for(Platform::linux_amd64()).await
    }
    async fn server_for(platform: Platform) -> Server {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}/", listener.local_addr().unwrap());
        let (archive, inventory) = fixture_for(&platform);
        let (archive_name, inventory_name) = names_for("v0.2.0", &platform);
        let assets = vec![(archive_name,&archive),(inventory_name,&inventory)].into_iter().map(|(name,bytes)|json!({"name":name,"size":bytes.len(),"digest":kimage_core::oci::digest(bytes),"state":"uploaded","browser_download_url":format!("{origin}download/v0.2.0/{name}")})).collect::<Vec<_>>();
        let state = Arc::new(Mutex::new(State {
            metadata: json!({"tag_name":"v0.2.0","draft":false,"assets":assets}),
            archive,
            failure: None,
            requests: 0,
            authenticated_metadata: 0,
        }));
        let shared = state.clone();
        let router = Router::new().fallback(move |request: axum::extract::Request| {
            let state = shared.clone();
            async move {
                let mut state = state.lock().unwrap();
                state.requests += 1;
                if request.uri().path().starts_with("/api/") {
                    if let Some(value) = request.headers().get("authorization") {
                        assert_eq!(value, "Bearer fixture-only");
                        state.authenticated_metadata += 1;
                    }
                    return axum::Json(state.metadata.clone()).into_response();
                }
                assert!(!request.headers().contains_key("authorization"));
                let mut body = state.archive.clone();
                if state.failure == Some("digest") {
                    body[0] ^= 1;
                }
                if state.failure == Some("truncate") {
                    let stream = futures::stream::iter(vec![
                        Ok(body[..body.len() / 2].to_vec()),
                        Err(std::io::Error::other("interrupted fixture")),
                    ]);
                    return Response::builder()
                        .status(StatusCode::OK)
                        .header("content-length", body.len())
                        .body(Body::from_stream(stream))
                        .unwrap();
                }
                Response::builder()
                    .status(StatusCode::OK)
                    .body(Body::from(body))
                    .unwrap()
            }
        });
        let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Server {
            client: Client {
                platform,
                http: reqwest::Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .build()
                    .unwrap(),
                api: format!("{origin}api/").parse().unwrap(),
                assets: format!("{origin}download/").parse().unwrap(),
                api_token: None,
            },
            state,
            task,
        }
    }

    #[tokio::test]
    async fn native_bundle_selection_verifies_arm_elf_oci_and_separates_cache_platforms() {
        let cache = tempfile::tempdir().unwrap();
        for platform in [Platform::linux_amd64(), Platform::linux_arm64()] {
            let server = server_for(platform.clone()).await;
            let directory = cache
                .path()
                .join(format!("v0.2.0-linux-{}", platform.architecture));
            let bundle = server
                .client
                .fetch(cache.path(), &directory, "v0.2.0", None)
                .await
                .unwrap();
            assert_eq!(bundle.inventory.platform, platform);
            drop(server);
            let offline = resolve_for_platform(
                None,
                Some(&bundle.digest),
                Some("0.2.0"),
                Some(cache.path()),
                true,
                &platform,
            )
            .await
            .unwrap();
            assert_eq!(offline.inventory.platform, platform);
        }
        let arm_directory = cache.path().join("v0.2.0-linux-arm64");
        let amd_directory = cache.path().join("v0.2.0-linux-amd64");
        fs::remove_dir_all(&arm_directory).unwrap();
        fs::rename(&amd_directory, &arm_directory).unwrap();
        assert!(
            resolve_for_platform(
                None,
                None,
                Some("0.2.0"),
                Some(cache.path()),
                true,
                &Platform::linux_arm64()
            )
            .await
            .is_err()
        );
    }

    #[test]
    fn exact_versions_and_release_origins_are_required() {
        for version in ["0.2.0", "v0.2.0", "0.2.0-rc.1"] {
            assert!(release_tag(version).is_ok());
        }
        for version in [
            "latest",
            "v0.2",
            "^0.2.0",
            "../0.2.0",
            "0.2.0+build",
            "v0.2.0/path",
        ] {
            assert!(release_tag(version).is_err());
        }
        for url in [
            "https://github.com/a",
            "https://release-assets.githubusercontent.com/a",
        ] {
            assert!(trusted_redirect(&url.parse().unwrap()));
        }
        for url in [
            "http://github.com/a",
            "https://github.com.evil.invalid/a",
            "https://github.com:8443/a",
            "https://user@github.com/a",
        ] {
            assert!(!trusted_redirect(&url.parse().unwrap()));
        }
    }

    #[tokio::test]
    async fn optional_token_is_sent_only_to_metadata_never_asset_downloads() {
        let mut server = server().await;
        server.client.api_token = Some(reqwest::header::HeaderValue::from_static(
            "Bearer fixture-only",
        ));
        let cache = tempfile::tempdir().unwrap();
        server
            .client
            .fetch(
                cache.path(),
                &cache.path().join("v0.2.0-linux-amd64"),
                "v0.2.0",
                None,
            )
            .await
            .unwrap();
        assert_eq!(server.state.lock().unwrap().authenticated_metadata, 1);
        assert_eq!(server.state.lock().unwrap().requests, 2);
    }

    #[tokio::test]
    async fn official_download_is_verified_cached_and_usable_offline() {
        let server = server().await;
        let cache = tempfile::tempdir().unwrap();
        let directory = cache.path().join("v0.2.0-linux-amd64");
        let bundle = server
            .client
            .fetch(cache.path(), &directory, "v0.2.0", None)
            .await
            .unwrap();
        let trusted = bundle.digest.clone();
        assert_eq!(server.state.lock().unwrap().requests, 2);
        drop(server);
        let restored = resolve(
            None,
            Some(&trusted),
            Some("0.2.0"),
            Some(cache.path()),
            true,
        )
        .await
        .unwrap();
        assert_eq!(restored.digest, trusted);
        assert!(
            resolve(
                None,
                Some(&format!("sha256:{}", "0".repeat(64))),
                Some("0.2.0"),
                Some(cache.path()),
                true
            )
            .await
            .is_err()
        );
        let executable = restored.root.join("kimage-input");
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(executable, b"corrupt cached executable").unwrap();
        assert!(
            resolve(None, None, Some("0.2.0"), Some(cache.path()), true)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn metadata_rejects_untrusted_or_incomplete_assets_before_download() {
        let server = server().await;
        let original = server.state.lock().unwrap().metadata.clone();
        for case in 0..7 {
            let mut value = original.clone();
            match case {
                0 => value["tag_name"] = json!("v9.0.0"),
                1 => value["draft"] = json!(true),
                2 => value["assets"][0]["digest"] = Value::Null,
                3 => {
                    value["assets"][0]["browser_download_url"] =
                        json!("https://evil.invalid/archive")
                }
                4 => value["assets"][0]["size"] = json!(MAX_ARCHIVE + 1),
                5 => {
                    let duplicate = value["assets"][0].clone();
                    value["assets"].as_array_mut().unwrap().push(duplicate);
                }
                _ => value["assets"][0]["state"] = json!("new"),
            }
            server.state.lock().unwrap().metadata = value;
            assert!(
                server.client.metadata("v0.2.0").await.is_err(),
                "case {case}"
            );
        }
        assert_eq!(server.state.lock().unwrap().requests, 7);
    }

    #[tokio::test]
    async fn interrupted_or_corrupt_download_never_commits_partial_cache() {
        let server = server().await;
        let cache = tempfile::tempdir().unwrap();
        let directory = cache.path().join("v0.2.0-linux-amd64");
        for failure in ["digest", "truncate"] {
            server.state.lock().unwrap().failure = Some(failure);
            assert!(
                server
                    .client
                    .fetch(cache.path(), &directory, "v0.2.0", None)
                    .await
                    .is_err()
            );
            assert!(!directory.exists());
            assert_eq!(fs::read_dir(cache.path()).unwrap().count(), 0);
        }
        server.state.lock().unwrap().failure = None;
        assert!(
            server
                .client
                .fetch(cache.path(), &directory, "v0.2.0", None)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn offline_miss_and_local_without_pin_fail_without_network_or_cache_writes() {
        let root = tempfile::tempdir().unwrap();
        let missing = root.path().join("absent");
        assert!(
            resolve(None, None, Some("0.2.0"), Some(&missing), true)
                .await
                .is_err()
        );
        assert!(!missing.exists());
        assert!(
            resolve(Some(root.path()), None, None, None, true)
                .await
                .is_err()
        );
        assert!(
            resolve(
                Some(root.path()),
                Some("invalid"),
                Some("0.2.0"),
                None,
                false
            )
            .await
            .is_err()
        );
    }

    #[test]
    fn archive_extraction_rejects_paths_links_duplicates_and_truncation() {
        for (name, kind) in [
            ("../escape", tar::EntryType::Regular),
            ("/absolute", tar::EntryType::Regular),
            ("link", tar::EntryType::Symlink),
            ("directory", tar::EntryType::Directory),
        ] {
            let mut tar = tar::Builder::new(Vec::new());
            let mut header = tar::Header::new_gnu();
            header.as_mut_bytes()[..name.len()].copy_from_slice(name.as_bytes());
            header.set_entry_type(kind);
            header.set_size(0);
            header.set_cksum();
            tar.append(&header, &[][..]).unwrap();
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("archive.gz");
            fs::write(&path, gzip(&tar.into_inner().unwrap())).unwrap();
            let output = root.path().join("output");
            fs::create_dir(&output).unwrap();
            assert!(extract(&path, &output).is_err(), "{name}");
            assert!(!root.path().join("escape").exists());
        }
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("archive.gz");
        let duplicate = gzip(&tar_bytes(&[
            ("inventory.json".into(), b"a".to_vec()),
            ("inventory.json".into(), b"b".to_vec()),
        ]));
        fs::write(&path, duplicate).unwrap();
        let output = root.path().join("duplicate");
        fs::create_dir(&output).unwrap();
        assert!(extract(&path, &output).is_err());
        let (mut archive, _) = fixture();
        archive.truncate(archive.len() - 4);
        fs::write(&path, archive).unwrap();
        let output = root.path().join("truncated");
        fs::create_dir(&output).unwrap();
        assert!(extract(&path, &output).is_err());
    }
}

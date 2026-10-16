//! Standard OCI Distribution publication, followed by independent byte readback.
use anyhow::{Context, Result, ensure};
use futures::StreamExt;
use kimage_core::oci::{Descriptor, Platform, VerifiedImage};
use reqwest::{Client, StatusCode, Url};
use sha2::{Digest, Sha256};
use std::{net::SocketAddr, path::Path, time::Duration};
use tokio_util::sync::CancellationToken;

/// Only successful complete byte readback can construct this publication receipt.
pub struct PublicationReceipt {
    root: Descriptor,
}
impl PublicationReceipt {
    pub fn root(&self) -> &Descriptor {
        &self.root
    }
}

pub struct Registry {
    client: Client,
    origin: Url,
    repository: String,
    username: String,
    password: String,
}
impl Registry {
    pub fn new(
        origin: Url,
        repository: String,
        ca: &[u8],
        resolve: Option<SocketAddr>,
        username: String,
        password: String,
    ) -> Result<Self> {
        Self::connect(origin, repository, Some(ca), resolve, username, password)
    }
    pub fn external(
        origin: Url,
        repository: String,
        ca: Option<&[u8]>,
        credentials: Option<(String, String)>,
    ) -> Result<Self> {
        let (username, password) = credentials.unwrap_or_default();
        Self::connect(origin, repository, ca, None, username, password)
    }
    fn connect(
        mut origin: Url,
        repository: String,
        ca: Option<&[u8]>,
        resolve: Option<SocketAddr>,
        username: String,
        password: String,
    ) -> Result<Self> {
        ensure!(
            origin.scheme() == "https"
                && origin.username().is_empty()
                && origin.password().is_none()
                && origin.path() == "/"
                && origin.query().is_none()
                && origin.fragment().is_none(),
            "registry must be an HTTPS origin"
        );
        ensure!(
            !repository.is_empty()
                && repository.len() <= 255
                && repository
                    .split('/')
                    .all(crate::export_api::repository_component),
            "invalid repository"
        );
        let mut builder = Client::builder()
            .https_only(true)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(180));
        if let Some(ca) = ca {
            builder = builder.add_root_certificate(reqwest::Certificate::from_pem(ca)?);
        }
        if let Some(address) = resolve {
            ensure!(
                address.ip().is_loopback() && address.port() != 0,
                "InvalidTunnelAddress"
            );
            // reqwest DNS overrides replace IPs, never an explicit URI port.
            // Keep the TLS hostname and use the API tunnel's actual local port.
            origin
                .set_port(Some(address.port()))
                .map_err(|_| anyhow::anyhow!("InvalidTunnelPort"))?;
            builder = builder.resolve(origin.host_str().context("missing registry host")?, address);
        }
        Ok(Self {
            client: builder.build()?,
            origin,
            repository,
            username,
            password,
        })
    }
    fn url(&self, suffix: &str) -> Result<Url> {
        Ok(self
            .origin
            .join(&format!("v2/{}/{suffix}", self.repository))?)
    }
    fn auth(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if self.username.is_empty() {
            request
        } else {
            request.basic_auth(&self.username, Some(&self.password))
        }
    }
    /// Tags have no portable CAS. Callers serialize their own writers, check
    /// before PUT, and verify afterward; other writers need registry enforcement.
    pub async fn tag_digest(&self, tag: &str) -> Result<Option<String>> {
        ensure!(crate::export_api::tag(tag), "InvalidExportTag");
        let response = self
            .auth(self.client.get(self.url(&format!("manifests/{tag}"))?))
            .header(
                "Accept",
                format!(
                    "{}, {}",
                    kimage_core::oci::MANIFEST,
                    kimage_core::oci::INDEX
                ),
            )
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("ExportTransportFailed"))?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        ensure!(
            response.status() != StatusCode::UNAUTHORIZED
                && response.status() != StatusCode::FORBIDDEN,
            "ExportAuthenticationFailed"
        );
        ensure!(response.status() == StatusCode::OK, "ExportTagReadFailed");
        let content_type = response
            .headers()
            .get("Content-Type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("");
        ensure!(
            matches!(
                content_type,
                kimage_core::oci::MANIFEST | kimage_core::oci::INDEX
            ),
            "UnsupportedRemoteMediaType"
        );
        let claimed = response
            .headers()
            .get("Docker-Content-Digest")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| anyhow::anyhow!("ExportReadInterrupted"))?;
            ensure!(
                bytes.len() + chunk.len() <= 2 * 1024 * 1024,
                "RemoteManifestTooLarge"
            );
            bytes.extend_from_slice(&chunk);
        }
        let digest = kimage_core::oci::digest(&bytes);
        ensure!(
            claimed.is_none_or(|s| s == digest),
            "RemoteTagDigestMismatch"
        );
        Ok(Some(digest))
    }
    pub async fn tag_never(&self, image: &VerifiedImage, tag: &str) -> Result<()> {
        if let Some(digest) = self.tag_digest(tag).await? {
            ensure!(digest == image.root().digest, "TagConflict");
            return Ok(());
        }
        let response = self
            .auth(self.client.put(self.url(&format!("manifests/{tag}"))?))
            .header("Content-Type", &image.root().media_type)
            .body(image.bytes(image.root())?)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("ExportTransportFailed"))?;
        ensure!(
            response.status() == StatusCode::CREATED,
            "ExportTagPublicationFailed"
        );
        check_digest_header(response.headers(), image.root())?;
        ensure!(
            self.tag_digest(tag).await?.as_deref() == Some(&image.root().digest),
            "RemoteTagChanged"
        );
        Ok(())
    }
    pub async fn publish(
        &self,
        image: &VerifiedImage,
        cancel: &CancellationToken,
    ) -> Result<PublicationReceipt> {
        let mut measured = crate::metrics::Observation::begin(crate::metrics::Stage::Publication);
        tokio::select! {
            _ = cancel.cancelled() => anyhow::bail!("PublicationCancelled"),
            result = tokio::time::timeout(Duration::from_secs(600), self.publish_retry(image)) => {
                result.context("PublicationDeadlineExceeded")??;
                measured.success();
                Ok(PublicationReceipt { root: image.root().clone() })
            }
        }
    }
    async fn publish_retry(&self, image: &VerifiedImage) -> Result<()> {
        for attempt in 0..3 {
            match self.publish_inner(image).await {
                Ok(()) => return Ok(()),
                Err(error)
                    if attempt < 2
                        && matches!(
                            error.to_string().as_str(),
                            "RegistryTransportFailed"
                                | "BlobChunkTransportFailed"
                                | "RegistryReadInterrupted"
                        ) =>
                {
                    // A lost response can follow an accepted PATCH. Restart the
                    // graph with HEAD/readback and a fresh upload session rather
                    // than replaying an ambiguous chunk at the previous offset.
                    tracing::warn!(attempt = attempt + 1, reason = %error, "retrying interrupted registry publication");
                    tokio::time::sleep(Duration::from_secs(1 << attempt)).await;
                }
                Err(error) => return Err(error),
            }
        }
        unreachable!()
    }
    /// Recover publication from retained registry bytes after a crash between
    /// immutable release creation and its verified-status acknowledgment.
    /// No original archive, source PVC, worker or cache is required.
    pub async fn fetch_verified(
        &self,
        root: &Descriptor,
        platform: &Platform,
        cancel: &CancellationToken,
    ) -> Result<VerifiedImage> {
        tokio::select! {
            _ = cancel.cancelled() => anyhow::bail!("ArtifactReadCancelled"),
            result = tokio::time::timeout(Duration::from_secs(600), self.fetch_inner(root, platform)) => result.context("ArtifactReadDeadlineExceeded")?,
        }
    }
    async fn fetch_inner(&self, root: &Descriptor, platform: &Platform) -> Result<VerifiedImage> {
        use std::collections::{BTreeMap, VecDeque};
        let spool = tempfile::tempdir()?;
        let mut queue = VecDeque::from([(root.clone(), 0_usize)]);
        let mut fetched: BTreeMap<String, (Descriptor, std::path::PathBuf)> = BTreeMap::new();
        let mut total = 0_u64;
        while let Some((descriptor, depth)) = queue.pop_front() {
            ensure!(depth <= 8 && fetched.len() < 4096, "ArtifactGraphLimit");
            ensure!(
                kimage_core::oci::valid_digest(&descriptor.digest)
                    && descriptor.size <= 512 * 1024 * 1024,
                "InvalidDescriptor"
            );
            if let Some((previous, _)) = fetched.get(&descriptor.digest) {
                ensure!(previous == &descriptor, "InconsistentDescriptor");
                continue;
            }
            total = total
                .checked_add(descriptor.size)
                .context("ArtifactSizeOverflow")?;
            ensure!(total <= 1024 * 1024 * 1024, "ArtifactTooLarge");
            let manifest = matches!(
                descriptor.media_type.as_str(),
                kimage_core::oci::INDEX | kimage_core::oci::MANIFEST
            );
            let kind = if manifest { "manifests" } else { "blobs" };
            let response = self
                .auth(
                    self.client
                        .get(self.url(&format!("{kind}/{}", descriptor.digest))?),
                )
                .header("Accept", &descriptor.media_type)
                .send()
                .await
                .map_err(|_| anyhow::anyhow!("RegistryTransportFailed"))?;
            ensure!(
                response.status() == StatusCode::OK
                    && response.content_length() == Some(descriptor.size),
                "ArtifactReadFailed"
            );
            check_digest_header(response.headers(), &descriptor)?;
            let path = spool.path().join(&descriptor.digest[7..]);
            let mut file = tokio::fs::File::create(&path).await?;
            let mut stream = response.bytes_stream();
            let mut hash = Sha256::new();
            let mut size = 0_u64;
            use tokio::io::AsyncWriteExt;
            while let Some(bytes) = stream.next().await {
                let bytes = bytes.map_err(|_| anyhow::anyhow!("RegistryReadInterrupted"))?;
                size += bytes.len() as u64;
                ensure!(size <= descriptor.size, "ArtifactSizeMismatch");
                hash.update(&bytes);
                file.write_all(&bytes).await?;
            }
            ensure!(
                size == descriptor.size
                    && format!("sha256:{}", hex::encode(hash.finalize())) == descriptor.digest,
                "ArtifactDigestMismatch"
            );
            file.sync_all().await?;
            drop(file);
            if manifest {
                ensure!(descriptor.size <= 2 * 1024 * 1024, "MetadataTooLarge");
                let content: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
                if descriptor.media_type == kimage_core::oci::INDEX {
                    let children: Vec<Descriptor> =
                        serde_json::from_value(content["manifests"].clone())?;
                    ensure!(
                        !children.is_empty() && children.len() <= 2,
                        "InvalidImageIndex"
                    );
                    queue.extend(children.into_iter().map(|d| (d, depth + 1)));
                } else {
                    let config: Descriptor = serde_json::from_value(content["config"].clone())?;
                    let layers: Vec<Descriptor> =
                        serde_json::from_value(content["layers"].clone())?;
                    queue.push_back((config, depth + 1));
                    queue.extend(layers.into_iter().map(|d| (d, depth + 1)));
                }
            }
            fetched.insert(descriptor.digest.clone(), (descriptor, path));
        }
        let archive = spool.path().join("image.tar");
        let expected = root.clone();
        let platform = platform.clone();
        tokio::task::spawn_blocking(move || {
            let mut tar = tar::Builder::new(std::fs::File::create(&archive)?);
            let mut append = |name: &str, bytes: &[u8]| -> Result<()> {
                let mut header = tar::Header::new_gnu();
                header.set_size(bytes.len() as u64);
                header.set_mode(0o444);
                header.set_cksum();
                tar.append_data(&mut header, name, bytes)?;
                Ok(())
            };
            append("oci-layout", br#"{"imageLayoutVersion":"1.0.0"}"#)?;
            append(
                "index.json",
                &serde_json::to_vec(
                    &serde_json::json!({"schemaVersion":2,"manifests":[expected]}),
                )?,
            )?;
            for (digest, (_, path)) in fetched {
                let mut file = std::fs::File::open(path)?;
                let mut header = tar::Header::new_gnu();
                header.set_size(file.metadata()?.len());
                header.set_mode(0o444);
                header.set_cksum();
                tar.append_data(
                    &mut header,
                    format!("blobs/sha256/{}", &digest[7..]),
                    &mut file,
                )?;
            }
            tar.finish()?;
            drop(tar);
            let verified = kimage_core::oci::verify_archive(
                &archive,
                &expected,
                &platform,
                kimage_core::oci::Limits::default(),
            );
            drop(spool);
            verified
        })
        .await?
    }
    async fn publish_inner(&self, image: &VerifiedImage) -> Result<()> {
        for blob in image.blobs() {
            self.upload_blob(blob, image.path(blob)?).await?;
        }
        for manifest in image.manifests() {
            let content = image.bytes(manifest)?;
            let size = content.len();
            let response = self
                .auth(
                    self.client
                        .put(self.url(&format!("manifests/{}", manifest.digest))?),
                )
                .header("Content-Type", &manifest.media_type)
                .body(content)
                .send()
                .await
                .map_err(|_| anyhow::anyhow!("RegistryTransportFailed"))?;
            ensure!(
                response.status() == StatusCode::CREATED,
                "ManifestPublicationFailed: {}",
                response.status()
            );
            check_digest_header(response.headers(), manifest)?;
            crate::metrics::transferred(crate::metrics::Stage::Publication, size);
            self.verify_remote(manifest, true).await?;
        }
        Ok(())
    }
    async fn upload_blob(&self, descriptor: &Descriptor, path: &Path) -> Result<()> {
        let head = self
            .auth(
                self.client
                    .head(self.url(&format!("blobs/{}", descriptor.digest))?),
            )
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("RegistryTransportFailed"))?;
        if head.status() == StatusCode::OK {
            self.verify_remote(descriptor, false).await?;
            return Ok(());
        }
        ensure!(
            head.status() == StatusCode::NOT_FOUND,
            "BlobHeadFailed: {}",
            head.status()
        );
        let response = self
            .auth(self.client.post(self.url("blobs/uploads/")?))
            .header("Content-Length", 0)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("RegistryTransportFailed"))?;
        ensure!(
            response.status() == StatusCode::ACCEPTED,
            "BlobUploadStartFailed: {}",
            response.status()
        );
        let location = response
            .headers()
            .get("Location")
            .context("MissingUploadLocation")?
            .to_str()?;
        let mut location = self.origin.join(location)?;
        // Never forward publication credentials to redirects or another origin.
        ensure!(
            location.origin() == self.origin.origin()
                && location
                    .path()
                    .starts_with(&format!("/v2/{}/blobs/uploads/", self.repository)),
            "UnsafeUploadLocation"
        );
        // Bound each request independently. A whole-layer PUT can exceed gateway
        // deadlines on a slow API tunnel even when the layer is within OCI limits.
        use tokio::io::AsyncReadExt;
        let mut file = tokio::fs::File::open(path).await?;
        let mut offset = 0_u64;
        while offset < descriptor.size {
            let size = (descriptor.size - offset).min(8 * 1024 * 1024) as usize;
            let mut bytes = vec![0_u8; size];
            file.read_exact(&mut bytes).await?;
            let response = self
                .auth(self.client.patch(location.clone()))
                .header("Content-Type", "application/octet-stream")
                .header("Content-Length", size)
                .header(
                    "Content-Range",
                    format!("{offset}-{}", offset + size as u64 - 1),
                )
                .body(bytes)
                .send()
                .await
                .map_err(|_| anyhow::anyhow!("BlobChunkTransportFailed"))?;
            ensure!(
                response.status() == StatusCode::ACCEPTED,
                "BlobChunkUploadFailed: {}",
                response.status()
            );
            offset += size as u64;
            crate::metrics::transferred(crate::metrics::Stage::Publication, size);
            ensure!(
                response
                    .headers()
                    .get("Range")
                    .and_then(|v| v.to_str().ok())
                    == Some(format!("0-{}", offset - 1).as_str()),
                "BlobChunkRangeMismatch"
            );
            location = self.origin.join(
                response
                    .headers()
                    .get("Location")
                    .context("MissingUploadLocation")?
                    .to_str()?,
            )?;
            ensure!(
                location.origin() == self.origin.origin()
                    && location
                        .path()
                        .starts_with(&format!("/v2/{}/blobs/uploads/", self.repository)),
                "UnsafeUploadLocation"
            );
        }
        location
            .query_pairs_mut()
            .append_pair("digest", &descriptor.digest);
        let response = self
            .auth(self.client.put(location))
            .header("Content-Type", "application/octet-stream")
            .header("Content-Length", 0)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("RegistryTransportFailed"))?;
        ensure!(
            response.status() == StatusCode::CREATED,
            "BlobUploadFailed: {}",
            response.status()
        );
        check_digest_header(response.headers(), descriptor)?;
        self.verify_remote(descriptor, false).await
    }
    /// Bounded authenticated manifest readback for installation health checks.
    pub async fn verify_retained_root(&self, root: &Descriptor) -> Result<()> {
        ensure!(
            matches!(
                root.media_type.as_str(),
                kimage_core::oci::INDEX | kimage_core::oci::MANIFEST
            ) && kimage_core::oci::valid_digest(&root.digest)
                && root.size <= 1024 * 1024,
            "InvalidRetainedRoot"
        );
        self.verify_remote(root, true).await
    }
    async fn verify_remote(&self, descriptor: &Descriptor, manifest: bool) -> Result<()> {
        let kind = if manifest { "manifests" } else { "blobs" };
        let response = self
            .auth(
                self.client
                    .get(self.url(&format!("{kind}/{}", descriptor.digest))?),
            )
            .header("Accept", &descriptor.media_type)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("RegistryTransportFailed"))?;
        ensure!(
            response.status() == StatusCode::OK,
            "RegistryReadbackFailed: {}",
            response.status()
        );
        check_digest_header(response.headers(), descriptor)?;
        ensure!(
            response.content_length() == Some(descriptor.size),
            "RegistrySizeMismatch"
        );
        let mut stream = response.bytes_stream();
        let mut hash = Sha256::new();
        let mut size = 0_u64;
        while let Some(bytes) = stream.next().await {
            let bytes = bytes.map_err(|_| anyhow::anyhow!("RegistryReadInterrupted"))?;
            crate::metrics::transferred(crate::metrics::Stage::Publication, bytes.len());
            size += bytes.len() as u64;
            ensure!(size <= descriptor.size, "RegistrySizeMismatch");
            hash.update(&bytes);
        }
        ensure!(
            size == descriptor.size
                && format!("sha256:{}", hex::encode(hash.finalize())) == descriptor.digest,
            "RegistryDigestMismatch"
        );
        Ok(())
    }
}
fn check_digest_header(
    headers: &reqwest::header::HeaderMap,
    descriptor: &Descriptor,
) -> Result<()> {
    ensure!(
        headers
            .get("Docker-Content-Digest")
            .is_none_or(|v| v.to_str().ok() == Some(descriptor.digest.as_str())),
        "RegistryDigestHeaderMismatch"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kimage_core::oci::{self, CONFIG, MANIFEST};
    use serde_json::json;
    use std::{
        collections::BTreeMap,
        sync::{Arc, Mutex},
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn image() -> VerifiedImage {
        let config = serde_json::to_vec(
            &json!({"os":"linux","architecture":"arm64","rootfs":{"type":"layers","diff_ids":[]}}),
        )
        .unwrap();
        let manifest = serde_json::to_vec(&json!({"schemaVersion":2,"mediaType":MANIFEST,"config":oci::descriptor(&config,CONFIG),"layers":[]})).unwrap();
        let root = oci::descriptor(&manifest, MANIFEST);
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut tar = tar::Builder::new(std::fs::File::create(file.path()).unwrap());
        let mut entries = vec![
            (
                "oci-layout".to_owned(),
                br#"{"imageLayoutVersion":"1.0.0"}"#.to_vec(),
            ),
            (
                "index.json".to_owned(),
                serde_json::to_vec(&json!({"schemaVersion":2,"manifests":[root]})).unwrap(),
            ),
        ];
        for bytes in [config, manifest] {
            entries.push((format!("blobs/sha256/{}", &oci::digest(&bytes)[7..]), bytes));
        }
        for (name, bytes) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o444);
            header.set_cksum();
            tar.append_data(&mut header, name, bytes.as_slice())
                .unwrap();
        }
        tar.finish().unwrap();
        drop(tar);
        oci::verify_archive(
            file.path(),
            &root,
            &Platform::linux_arm64(),
            oci::Limits::default(),
        )
        .unwrap()
    }

    #[derive(Default)]
    struct Remote {
        requests: usize,
        starts: usize,
        uploads: BTreeMap<String, Vec<u8>>,
        blobs: BTreeMap<String, Vec<u8>>,
        manifests: BTreeMap<String, Vec<u8>>,
    }
    async fn publication_case(
        disconnects: usize,
        corrupt_readback: bool,
        deny: bool,
    ) -> (Result<PublicationReceipt>, usize, usize) {
        let state = Arc::new(Mutex::new(Remote::default()));
        let remote = state.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut header = Vec::new();
                while !header.ends_with(b"\r\n\r\n") {
                    header.push(socket.read_u8().await.unwrap());
                    assert!(header.len() <= 16384);
                }
                let text = String::from_utf8(header).unwrap();
                let mut request = text.lines().next().unwrap().split_whitespace();
                let method = request.next().unwrap();
                let target = request.next().unwrap();
                let length: usize = text
                    .lines()
                    .filter_map(|line| line.split_once(':'))
                    .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                    .map(|(_, value)| value.trim().parse().unwrap())
                    .unwrap_or(0);
                assert!(length <= 16384);
                let mut bytes = vec![0; length];
                socket.read_exact(&mut bytes).await.unwrap();
                let response = {
                    let mut state = remote.lock().unwrap();
                    state.requests += 1;
                    let url = Url::parse(&format!("http://localhost{target}")).unwrap();
                    let path = url.path().strip_prefix("/v2/apps/image/").unwrap();
                    if deny {
                        Some((403, Vec::new(), Vec::new()))
                    } else if method == "POST" && path == "blobs/uploads/" {
                        state.starts += 1;
                        let id = state.starts.to_string();
                        state.uploads.insert(id.clone(), Vec::new());
                        Some((
                            202,
                            vec![("Location", format!("/v2/apps/image/blobs/uploads/{id}"))],
                            Vec::new(),
                        ))
                    } else if let Some(id) = path.strip_prefix("blobs/uploads/") {
                        if method == "PATCH" {
                            // Accept bytes, then lose the response. Replaying
                            // this PATCH in the same session would duplicate data.
                            let upload = state.uploads.get_mut(id).unwrap();
                            upload.extend(bytes);
                            let end = upload.len() - 1;
                            if state.starts <= disconnects {
                                None
                            } else {
                                Some((
                                    202,
                                    vec![
                                        ("Location", format!("/v2/apps/image/blobs/uploads/{id}")),
                                        ("Range", format!("0-{end}")),
                                    ],
                                    Vec::new(),
                                ))
                            }
                        } else {
                            assert_eq!(method, "PUT");
                            let digest = url
                                .query_pairs()
                                .find(|(key, _)| key == "digest")
                                .unwrap()
                                .1
                                .into_owned();
                            let uploaded = state.uploads.remove(id).unwrap();
                            assert_eq!(oci::digest(&uploaded), digest);
                            state.blobs.insert(digest.clone(), uploaded);
                            Some((201, vec![("Docker-Content-Digest", digest)], Vec::new()))
                        }
                    } else if let Some(digest) = path.strip_prefix("manifests/") {
                        if method == "PUT" {
                            assert_eq!(oci::digest(&bytes), digest);
                            state.manifests.insert(digest.to_owned(), bytes);
                            Some((
                                201,
                                vec![("Docker-Content-Digest", digest.to_owned())],
                                Vec::new(),
                            ))
                        } else {
                            Some((
                                200,
                                vec![("Docker-Content-Digest", digest.to_owned())],
                                state.manifests[digest].clone(),
                            ))
                        }
                    } else {
                        let digest = path.strip_prefix("blobs/").unwrap();
                        if method == "HEAD" {
                            Some((
                                if state.blobs.contains_key(digest) {
                                    200
                                } else {
                                    404
                                },
                                Vec::new(),
                                Vec::new(),
                            ))
                        } else {
                            assert_eq!(method, "GET");
                            let mut bytes = state.blobs[digest].clone();
                            if corrupt_readback {
                                bytes[0] ^= 1;
                            }
                            Some((
                                200,
                                vec![("Docker-Content-Digest", digest.to_owned())],
                                bytes,
                            ))
                        }
                    }
                };
                if let Some((status, headers, bytes)) = response {
                    let mut response = format!(
                        "HTTP/1.1 {status} Fixture\r\nConnection: close\r\nContent-Length: {}\r\n",
                        bytes.len()
                    );
                    for (name, value) in headers {
                        response.push_str(&format!("{name}: {value}\r\n"));
                    }
                    response.push_str("\r\n");
                    socket.write_all(response.as_bytes()).await.unwrap();
                    socket.write_all(&bytes).await.unwrap();
                }
            }
        });
        // Only this private HTTP fixture bypasses the production HTTPS constructor.
        let registry = Registry {
            client: Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap(),
            origin: format!("http://{address}/").parse().unwrap(),
            repository: "apps/image".into(),
            username: String::new(),
            password: String::new(),
        };
        let image = image();
        let result = registry.publish(&image, &CancellationToken::new()).await;
        if let Ok(receipt) = &result {
            assert_eq!(receipt.root(), image.root());
        }
        let starts = state.lock().unwrap().starts;
        let requests = state.lock().unwrap().requests;
        server.abort();
        (result, starts, requests)
    }

    #[tokio::test]
    async fn interrupted_upload_restarts_session_but_integrity_auth_and_retry_limits_still_apply() {
        let (result, starts, _) = publication_case(1, false, false).await;
        result.unwrap();
        assert_eq!(starts, 2);
        let (result, starts, _) = publication_case(usize::MAX, false, false).await;
        assert_eq!(
            result.err().unwrap().to_string(),
            "BlobChunkTransportFailed"
        );
        assert_eq!(starts, 3);
        let (result, starts, _) = publication_case(1, true, false).await;
        assert_eq!(result.err().unwrap().to_string(), "RegistryDigestMismatch");
        assert_eq!(starts, 2);
        let (result, starts, requests) = publication_case(0, false, true).await;
        assert!(result.is_err());
        assert_eq!(starts, 0);
        assert_eq!(requests, 1);
    }
}

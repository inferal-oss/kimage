//! Tokenless, namespace-local full graph acceptance, using reader credentials.
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeSet, io::Read, path::Path};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    repository: String,
    root: kimage_core::oci::Descriptor,
}
fn read(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= limit, "RegistryCheckInputTooLarge");
    Ok(bytes)
}
pub async fn run(request: &Path, origin: reqwest::Url, credentials: &Path) -> Result<Value> {
    let requests: Vec<Request> = serde_json::from_slice(&read(request, 65536)?)?;
    ensure!(
        !requests.is_empty() && requests.len() <= 128,
        "InvalidRegistryCheckCount"
    );
    let ca = read(&credentials.join("ca.crt"), 65536)?;
    let password = String::from_utf8(read(&credentials.join("reader-password"), 256)?)
        .context("InvalidRegistryReader")?;
    let mut platforms = BTreeSet::new();
    for request in &requests {
        let registry = crate::registry::Registry::new(
            origin.clone(),
            request.repository.clone(),
            &ca,
            None,
            "reader".into(),
            password.trim_end().into(),
        )?;
        let image = registry
            .fetch_verified(
                &request.root,
                &kimage_core::oci::Platform::automatic(),
                &tokio_util::sync::CancellationToken::new(),
            )
            .await?;
        platforms.extend(
            image
                .platforms()
                .iter()
                .map(kimage_core::oci::Platform::name),
        );
    }
    Ok(
        json!({"complete":true,"verifiedReleases":requests.len(),"platforms":platforms,"verification":"authenticated TLS full OCI graph, descriptor sizes, SHA-256 and DiffIDs"}),
    )
}

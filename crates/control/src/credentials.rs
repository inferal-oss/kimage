//! Same-CA leaf renewal with prevalidation and verified projected-file reload.
use crate::{
    install::{Installer, LABEL},
    tunnel::Tunnel,
};
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use kube::{
    ResourceExt,
    api::{Patch, PatchParams},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path, sync::Arc, time::Duration};

fn file(directory: &Path, name: &str) -> Result<Vec<u8>> {
    use std::io::Read;
    let path = directory.join(name);
    let metadata = std::fs::symlink_metadata(&path)?;
    ensure!(
        metadata.is_file() && metadata.len() > 0 && metadata.len() <= 65536,
        "InvalidRenewalCredentialFile"
    );
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(65537)
        .read_to_end(&mut bytes)?;
    ensure!(
        !bytes.is_empty() && bytes.len() <= 65536,
        "InvalidRenewalCredentialSize"
    );
    Ok(bytes)
}
struct Leaf {
    role: &'static str,
    dns: String,
    values: BTreeMap<String, Vec<u8>>,
    der: Vec<u8>,
}
impl Leaf {
    fn load(role: &'static str, dns: String, directory: &Path, preserved: &[&str]) -> Result<Self> {
        let mut values = BTreeMap::new();
        for name in ["ca.crt", "tls.crt", "tls.key"]
            .into_iter()
            .chain(preserved.iter().copied())
        {
            values.insert(name.to_owned(), file(directory, name)?);
        }
        let der = rustls_pemfile::certs(&mut values["tls.crt"].as_slice())
            .next()
            .context("MissingRenewalLeaf")??
            .to_vec();
        Ok(Self {
            role,
            dns,
            values,
            der,
        })
    }
    fn client(&self, address: std::net::SocketAddr) -> Result<reqwest::Client> {
        Ok(reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .add_root_certificate(reqwest::Certificate::from_pem(&self.values["ca.crt"])?)
            .resolve(&self.dns, address)
            .tls_info(true)
            .timeout(Duration::from_secs(5))
            .build()?)
    }
    async fn validate(&self) -> Result<()> {
        use tokio_rustls::{TlsAcceptor, rustls};
        let _ = rustls::crypto::ring::default_provider().install_default();
        let certificates = rustls_pemfile::certs(&mut self.values["tls.crt"].as_slice())
            .collect::<std::io::Result<Vec<_>>>()?;
        let key = rustls_pemfile::private_key(&mut self.values["tls.key"].as_slice())?
            .context("MissingRenewalKey")?;
        let config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certificates, key)
            .context("InvalidRenewalKeyPair")?;
        let acceptor = TlsAcceptor::from(Arc::new(config));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let server = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let work = async move {
                let (stream, _) = listener.accept().await?;
                let mut stream = acceptor.accept(stream).await?;
                let mut header = [0_u8; 4096];
                let _ = stream.read(&mut header).await?;
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .await?;
                Ok::<_, std::io::Error>(())
            };
            let _ = tokio::time::timeout(Duration::from_secs(5), work).await;
        });
        let response = self
            .client(address)?
            .get(format!("https://{}:{}/readyz", self.dns, address.port()))
            .send()
            .await;
        server.abort();
        let response = response.map_err(|_| {
            anyhow::anyhow!("RenewalLeafValidationFailed: check CA, SAN, key usage and validity")
        })?;
        ensure!(
            response.status().is_success() && peer(&response) == Some(self.der.as_slice()),
            "RenewalLeafIdentityMismatch"
        );
        Ok(())
    }
}
fn peer(response: &reqwest::Response) -> Option<&[u8]> {
    response
        .extensions()
        .get::<reqwest::tls::TlsInfo>()?
        .peer_certificate()
}
pub(crate) async fn validate_registry_leaf(dns: String, directory: &Path) -> Result<()> {
    Leaf::load(
        "registry",
        dns,
        directory,
        &["reader-password", "publisher-password"],
    )?
    .validate()
    .await
}
impl Installer {
    pub async fn renew_tls(&self, admission_credentials: &Path) -> Result<Value> {
        self.preflight().await?;
        let registry = Leaf::load(
            "registry",
            format!("{}.{}.svc", self.name("registry"), self.options.namespace),
            &self.options.credentials,
            &["reader-password", "publisher-password"],
        )?;
        let admission = Leaf::load(
            "admission",
            format!("{}.{}.svc", self.name("admission"), self.options.namespace),
            admission_credentials,
            &["resolution-key"],
        )?;
        let api = self.api_at("Secret", Some(&self.options.namespace))?;
        let mut replacements = Vec::new();
        // Validate both complete identities before updating either Secret.
        for leaf in [registry, admission] {
            let name = self.name(&format!("{}-credentials", leaf.role));
            let current = api.get(&name).await?;
            ensure!(
                current.labels().get(LABEL) == Some(&self.options.installation),
                "RenewalSecretOwnershipConflict"
            );
            for (key, bytes) in &leaf.values {
                if key == "tls.crt" || key == "tls.key" {
                    continue;
                }
                ensure!(
                    current.data["data"][key] == STANDARD.encode(bytes),
                    "RenewalPreservedCredentialMismatch: CA/password/key changes use maintenance recovery"
                );
            }
            leaf.validate().await?;
            let pods = self.api_at("Pod", Some(&self.options.namespace))?;
            let pod = pods.get(&self.name(leaf.role)).await?;
            ensure!(
                pod.labels().get(LABEL) == Some(&self.options.installation),
                "RenewalPodOwnershipConflict"
            );
            let pod_uid = pod.uid().context("MissingRenewalPodUid")?;
            replacements.push((leaf, name, current, pod_uid));
        }
        for (leaf, name, current, _) in &replacements {
            api.patch(name,&PatchParams::default(),&Patch::Merge(json!({"metadata":{"uid":current.uid(),"resourceVersion":current.resource_version()},"data":{"tls.crt":STANDARD.encode(&leaf.values["tls.crt"]),"tls.key":STANDARD.encode(&leaf.values["tls.key"])}}))).await.map_err(|_|anyhow::anyhow!("RenewalSecretUpdateFailed"))?;
        }
        let mut receipts = Vec::new();
        for (leaf, _, _, pod_uid) in replacements {
            let port = if leaf.role == "registry" { 5443 } else { 8443 };
            let tunnel = Tunnel::open(
                self.client.clone(),
                &self.options.namespace,
                &self.name(leaf.role),
                port,
            )
            .await?;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
            loop {
                let path = if leaf.role == "registry" {
                    "v2/"
                } else {
                    "readyz"
                };
                let mut request = leaf.client(tunnel.address)?.get(format!(
                    "https://{}:{}/{path}",
                    leaf.dns,
                    tunnel.address.port()
                ));
                if leaf.role == "registry" {
                    request = request.basic_auth(
                        "reader",
                        Some(std::str::from_utf8(&leaf.values["reader-password"])?.trim_end()),
                    );
                }
                if let Ok(response) = request.send().await
                    && response.status().is_success()
                    && peer(&response) == Some(leaf.der.as_slice())
                {
                    break;
                }
                ensure!(
                    tokio::time::Instant::now() < deadline,
                    "RenewalReloadTimeout: resume with the same issued credentials"
                );
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            let pods = self.api_at("Pod", Some(&self.options.namespace))?;
            ensure!(
                pods.get(&self.name(leaf.role)).await?.uid().as_deref() == Some(&pod_uid),
                "RenewalPodChanged: verify current serving before retry"
            );
            receipts.push(json!({"role":leaf.role,"podUid":pod_uid,"certificateSha256":kimage_core::oci::digest(&leaf.der),"verifiedReload":true}));
        }
        Ok(json!({"renewed":receipts}))
    }
}

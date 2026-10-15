//! Preserve kubeconfig credentials and CA verification, with bounded API calls.
use anyhow::{Result, ensure};
use std::time::Duration;
pub async fn client() -> Result<kube::Client> {
    let mut config = kube::Config::infer().await?;
    ensure!(!config.accept_invalid_certs, "VerifiedApiTlsRequired");
    config.connect_timeout = Some(Duration::from_secs(10));
    config.read_timeout = Some(Duration::from_secs(30));
    match (
        std::env::var("KIMAGE_API_SERVER").ok(),
        std::env::var("KIMAGE_TLS_SERVER_NAME").ok(),
    ) {
        (Some(server), Some(name)) => {
            let url = reqwest::Url::parse(&server)?;
            ensure!(
                url.scheme() == "https"
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.query().is_none()
                    && url.fragment().is_none(),
                "InvalidApiServerOverride"
            );
            ensure!(
                kimage_core::selector::dns_subdomain(&name),
                "InvalidApiTlsName"
            );
            config.cluster_url = server.parse()?;
            config.tls_server_name = Some(name);
        }
        (None, None) => {}
        _ => anyhow::bail!("ApiServerOverrideRequiresTlsServerName"),
    }
    Ok(kube::Client::try_from(config)?)
}

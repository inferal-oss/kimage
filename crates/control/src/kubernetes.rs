//! Preserve kubeconfig credentials and CA verification without expiring streams.
use anyhow::{Result, ensure};
use std::time::Duration;
pub async fn client() -> Result<kube::Client> {
    let mut config = transport(kube::Config::infer().await?)?;
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
fn transport(mut config: kube::Config) -> Result<kube::Config> {
    ensure!(!config.accept_invalid_certs, "VerifiedApiTlsRequired");
    config.connect_timeout = Some(Duration::from_secs(10));
    // This is a socket timeout, not a per-request deadline. A quiet watch (or
    // exec/port-forward stream) is healthy and must survive it. kube-runtime
    // bounds watch inactivity itself; retain its transport default here.
    config.read_timeout = None;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::TryStreamExt;
    use k8s_openapi::api::core::v1::ConfigMap;
    use kube::{Api, api::WatchParams};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn quiet_watch_delivers_an_event_after_thirty_seconds() -> Result<()> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await?;
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                request.push(socket.read_u8().await?);
            }
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n").await?;
            socket.flush().await?;
            tokio::time::sleep(Duration::from_secs(32)).await;
            let event = b"{\"type\":\"ADDED\",\"object\":{\"apiVersion\":\"v1\",\"kind\":\"ConfigMap\",\"metadata\":{\"name\":\"after-idle\",\"namespace\":\"default\",\"resourceVersion\":\"2\"}}}\n";
            socket
                .write_all(format!("{:x}\r\n", event.len()).as_bytes())
                .await?;
            socket.write_all(event).await?;
            socket.write_all(b"\r\n0\r\n\r\n").await?;
            anyhow::Ok(())
        });
        let config = transport(kube::Config::new(format!("http://{address}").parse()?))?;
        let api: Api<ConfigMap> = Api::namespaced(kube::Client::try_from(config)?, "default");
        let events = api.watch(&WatchParams::default().timeout(60), "1").await?;
        futures::pin_mut!(events);
        let event = tokio::time::timeout(Duration::from_secs(40), events.try_next()).await??;
        assert!(
            matches!(event, Some(kube::api::WatchEvent::Added(value)) if value.metadata.name.as_deref() == Some("after-idle"))
        );
        server.await??;
        Ok(())
    }
}

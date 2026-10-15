//! Fixed upstream digest-only read gateway, with bounded streaming and no redirects.
use anyhow::{Result, ensure};
use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
};
use futures::StreamExt;
use std::{
    collections::BTreeSet,
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};
pub struct Proxy {
    client: reqwest::Client,
    upstream: reqwest::Url,
    namespaces: BTreeSet<String>,
    password_file: std::path::PathBuf,
    permits: Arc<tokio::sync::Semaphore>,
    endpoint_id: String,
    authority: String,
}
pub fn allowed(
    method: &Method,
    path: &str,
    query: Option<&str>,
    namespaces: &BTreeSet<String>,
) -> bool {
    if !matches!(*method, Method::GET | Method::HEAD) || query.is_some() {
        return false;
    }
    if path == "/v2/" {
        return true;
    }
    let Some(tail) = path.strip_prefix("/v2/") else {
        return false;
    };
    let pieces: Vec<_> = tail.split('/').collect();
    pieces.len() == 4
        && kimage_core::scope::registry_allows(namespaces, pieces[0])
        && valid_identity(pieces[1])
        && matches!(pieces[2], "blobs" | "manifests")
        && kimage_core::oci::valid_digest(pieces[3])
}
pub fn valid_identity(id: &str) -> bool {
    (id.len() == 36
        && id.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
            }
        }))
        || (id
            .strip_prefix("system-")
            .is_some_and(kimage_core::selector::dns_label))
}
impl Proxy {
    pub fn new(
        upstream: reqwest::Url,
        ca: &[u8],
        namespaces: BTreeSet<String>,
        password_file: std::path::PathBuf,
    ) -> Result<Self> {
        ensure!(
            upstream.scheme() == "https"
                && upstream.username().is_empty()
                && upstream.password().is_none()
                && upstream.path() == "/"
                && upstream.query().is_none()
                && upstream.fragment().is_none(),
            "InvalidUpstream"
        );
        ensure!(
            kimage_core::scope::valid_registry_namespaces(&namespaces),
            "InvalidNamespaceAllowlist"
        );
        let client = reqwest::Client::builder()
            .no_proxy()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .add_root_certificate(reqwest::Certificate::from_pem(ca)?)
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(120))
            .build()?;
        let endpoint_id = kimage_core::oci::digest(&serde_json::to_vec(
            &serde_json::json!({"upstream":upstream.as_str(),"namespaces":namespaces,"ca":kimage_core::oci::digest(ca)}),
        )?);
        Ok(Self {
            client,
            upstream,
            namespaces,
            password_file,
            permits: Arc::new(tokio::sync::Semaphore::new(16)),
            endpoint_id,
            authority: String::new(),
        })
    }
    pub async fn serve(mut self, listen: SocketAddr, overlap: bool) -> Result<()> {
        ensure!(
            listen.ip() == IpAddr::from([127, 0, 0, 1]) && listen.port() >= 1024,
            "ProxyMustBindLoopback"
        );
        self.authority = listen.to_string();
        use socket2::{Domain, Protocol, Socket, Type};
        let probe = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
        probe.set_reuse_address(true)?;
        if let Err(error) = probe.bind(&listen.into()) {
            ensure!(
                error.kind() == std::io::ErrorKind::AddrInUse,
                "EndpointBindFailed"
            );
            ensure!(overlap, "EndpointPortInUse");
            let mut nonce = [0_u8; 16];
            getrandom::fill(&mut nonce).map_err(|_| anyhow::anyhow!("EndpointNonceUnavailable"))?;
            let nonce = hex::encode(nonce);
            let client = reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(2))
                .build()?;
            let response = client
                .get(format!("http://{listen}/kimage/handover?nonce={nonce}"))
                .send()
                .await?;
            ensure!(response.status().is_success(), "ForeignEndpointPortOwner");
            let mut response = response;
            let mut text = Vec::new();
            while let Some(chunk) = response.chunk().await? {
                ensure!(text.len() + chunk.len() <= 128, "InvalidEndpointChallenge");
                text.extend_from_slice(&chunk);
            }
            verify_challenge(&self, &nonce, std::str::from_utf8(&text)?.trim())?;
        }
        drop(probe);
        let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
        socket.set_reuse_address(true)?;
        socket.set_reuse_port(true)?;
        socket.set_nonblocking(true)?;
        socket.bind(&listen.into())?;
        socket.listen(128)?;
        let listener = tokio::net::TcpListener::from_std(socket.into())?;
        axum::serve(
            listener,
            Router::new().fallback(read).with_state(Arc::new(self)),
        )
        .with_graceful_shutdown(crate::serving::shutdown())
        .await?;
        Ok(())
    }
}
fn challenge(p: &Proxy, nonce: &str) -> Result<String> {
    use base64::Engine;
    use hmac::{Hmac, Mac};
    let password = std::fs::read_to_string(&p.password_file)?;
    let mut mac =
        Hmac::<sha2::Sha256>::new_from_slice(password.trim_end().as_bytes()).expect("HMAC key");
    mac.update(b"kimage-compatible-endpoint-v1\0");
    mac.update(p.endpoint_id.as_bytes());
    mac.update(nonce.as_bytes());
    Ok(base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes()))
}
fn verify_challenge(p: &Proxy, nonce: &str, proof: &str) -> Result<()> {
    use base64::Engine;
    use hmac::{Hmac, Mac};
    let password = std::fs::read_to_string(&p.password_file)?;
    let mut mac =
        Hmac::<sha2::Sha256>::new_from_slice(password.trim_end().as_bytes()).expect("HMAC key");
    mac.update(b"kimage-compatible-endpoint-v1\0");
    mac.update(p.endpoint_id.as_bytes());
    mac.update(nonce.as_bytes());
    mac.verify_slice(&base64::engine::general_purpose::STANDARD.decode(proof)?)
        .map_err(|_| anyhow::anyhow!("ForeignEndpointPortOwner"))
}
async fn read(State(p): State<Arc<Proxy>>, r: Request) -> Response {
    let hosts: Vec<_> = r.headers().get_all("host").iter().collect();
    let canonical = if hosts.len() == 1 {
        hosts[0].as_bytes() == p.authority.as_bytes()
    } else if hosts.is_empty() {
        r.uri()
            .authority()
            .is_some_and(|a| a.as_str() == p.authority)
    } else {
        false
    };
    if !canonical {
        return StatusCode::FORBIDDEN.into_response();
    }
    // Fixed-cardinality local diagnostics; the authority check and loopback bind
    // apply equally here. No upstream credentials or paths enter metric labels.
    if r.uri().path() == "/metrics" && r.method() == Method::GET && r.uri().query().is_none() {
        return crate::metrics::render().await.into_response();
    }
    if r.uri().path() == "/kimage/handover" {
        if r.method() != Method::GET {
            return StatusCode::FORBIDDEN.into_response();
        }
        let Some(nonce) = r
            .uri()
            .query()
            .and_then(|q| q.strip_prefix("nonce="))
            .filter(|n| n.len() == 32 && n.bytes().all(|b| b.is_ascii_hexdigit()))
        else {
            return StatusCode::BAD_REQUEST.into_response();
        };
        return match challenge(&p, nonce) {
            Ok(value) => value.into_response(),
            Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
        };
    }
    if !allowed(r.method(), r.uri().path(), r.uri().query(), &p.namespaces) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let mut measured = crate::metrics::Observation::begin(crate::metrics::Stage::Pull);
    let Ok(permit) = p.permits.clone().try_acquire_owned() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let Ok(password) = std::fs::read_to_string(&p.password_file) else {
        return StatusCode::BAD_GATEWAY.into_response();
    };
    let Ok(url) = p.upstream.join(r.uri().path()) else {
        return StatusCode::BAD_GATEWAY.into_response();
    };
    let mut request = p
        .client
        .request(r.method().clone(), url)
        .basic_auth("reader", Some(password.trim_end()))
        // Service routing must take effect on the next immutable request during
        // handover, rather than retaining a connection to the retiring gateway.
        .header("connection", "close");
    if r.uri().path() == "/v2/" {
        request = request.timeout(Duration::from_secs(2));
    }
    for h in ["accept", "range", "if-range", "if-none-match"] {
        if let Some(value) = r.headers().get(h) {
            request = request.header(h, value);
        }
    }
    let retry = request.try_clone();
    let attempt = request.send().await;
    let attempt = match (attempt, retry) {
        (Err(_), Some(retry)) => retry.send().await,
        (result, _) => result,
    };
    let response = match attempt {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(reason="RegistryUpstreamUnavailable", error=%error.without_url(), "registry read deferred");
            return StatusCode::BAD_GATEWAY.into_response();
        }
    };
    if response.status().is_redirection()
        || matches!(
            response.status(),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        )
    {
        return StatusCode::BAD_GATEWAY.into_response();
    }
    tracing::info!(path=r.uri().path(),method=%r.method(),status=%response.status(),"registry read");
    let mut builder = Response::builder().status(response.status());
    for h in [
        "content-type",
        "content-length",
        "content-range",
        "accept-ranges",
        "docker-content-digest",
        "docker-distribution-api-version",
        "etag",
    ] {
        if let Some(v) = response.headers().get(h) {
            builder = builder.header(h, v);
        }
    }
    let successful = response.status().is_success();
    if r.method() == Method::HEAD {
        if successful {
            measured.success();
        }
        return builder
            .body(Body::empty())
            .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response());
    }
    let expected = response.content_length();
    let stream = response.bytes_stream().map(move |part| {
        let _ = &permit;
        part
    });
    let stream = crate::metrics::observe_stream(stream, measured, expected, successful);
    builder
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn overlap_proof_binds_nonce_credentials_and_upstream_configuration() {
        let dir = tempfile::tempdir().unwrap();
        let password = dir.path().join("password");
        std::fs::write(&password, "fixture-reader-key-32-bytes-minimum").unwrap();
        let ca = include_bytes!("../tests/fixtures/transport-ca.crt");
        let namespaces = ["app".to_owned()].into_iter().collect();
        let proxy = Proxy::new(
            "https://registry.fixture.svc:5443/".parse().unwrap(),
            ca,
            namespaces,
            password.clone(),
        )
        .unwrap();
        let nonce = "00112233445566778899aabbccddeeff";
        let proof = challenge(&proxy, nonce).unwrap();
        verify_challenge(&proxy, nonce, &proof).unwrap();
        assert!(verify_challenge(&proxy, "ffeeddccbbaa99887766554433221100", &proof).is_err());
        let other = Proxy::new(
            "https://other.fixture.svc:5443/".parse().unwrap(),
            ca,
            ["app".to_owned()].into_iter().collect(),
            password.clone(),
        )
        .unwrap();
        assert!(verify_challenge(&other, nonce, &proof).is_err());
        std::fs::write(&password, "rotated-fixture-reader-key-32-bytes").unwrap();
        assert!(verify_challenge(&proxy, nonce, &proof).is_err());
    }
    #[test]
    fn read_policy_cannot_tag_write_redirect_or_escape() {
        let ns = BTreeSet::from(["ns".into()]);
        let path = format!(
            "/v2/ns/00000000-1111-2222-3333-444444444444/manifests/{}",
            kimage_core::oci::digest(b"m")
        );
        assert!(allowed(&Method::GET, &path, None, &ns));
        assert!(allowed(&Method::HEAD, &path, None, &ns));
        assert!(!allowed(&Method::PUT, &path, None, &ns));
        assert!(!allowed(&Method::GET, &path, Some("x=1"), &ns));
        for path in [
            "/v2/ns/system-proxy/manifests/latest",
            "/v2/ns/%2e%2e/blobs/sha256:bad",
            "/v2/ns/id/blobs/uploads/",
        ] {
            assert!(!allowed(&Method::GET, path, None, &ns));
        }
    }
}

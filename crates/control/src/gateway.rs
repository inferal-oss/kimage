//! Distribution owns storage/protocol. This TLS gate independently restricts identities.
//! Distribution itself binds Pod loopback and is never exposed as an unauthenticated Service.
use anyhow::Result;
use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use futures::StreamExt;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::{collections::BTreeSet, path::PathBuf, sync::Arc, time::Duration};
struct Gate {
    client: reqwest::Client,
    namespaces: BTreeSet<String>,
    read: PathBuf,
    write: PathBuf,
    permits: Arc<tokio::sync::Semaphore>,
}
fn equals(expected: &[u8], provided: &[u8]) -> bool {
    let mut e =
        Hmac::<Sha256>::new_from_slice(b"kimage-constant-time-credential-comparison").unwrap();
    e.update(expected);
    let mut p =
        Hmac::<Sha256>::new_from_slice(b"kimage-constant-time-credential-comparison").unwrap();
    p.update(provided);
    p.verify_slice(&e.finalize().into_bytes()).is_ok()
}
/// Give application sessions read access to one namespace without exposing the
/// installation-wide reader password. Legacy system readers remain compatible.
pub fn namespace_reader_password(password: &str, namespace: &str) -> Result<String> {
    anyhow::ensure!(
        !password.is_empty()
            && password.len() <= 4096
            && kimage_core::selector::dns_label(namespace),
        "InvalidNamespaceReader"
    );
    let mut mac = Hmac::<Sha256>::new_from_slice(password.as_bytes())?;
    mac.update(b"kimage-namespace-reader-v1\0");
    mac.update(namespace.as_bytes());
    Ok(hex::encode(mac.finalize().into_bytes()))
}
fn credential_matches(name: &str, provided: &str, expected: &str) -> bool {
    if let Some(namespace) = name.strip_prefix("reader/") {
        namespace_reader_password(expected, namespace)
            .is_ok_and(|key| equals(key.as_bytes(), provided.as_bytes()))
    } else {
        matches!(name, "reader" | "publisher") && equals(expected.as_bytes(), provided.as_bytes())
    }
}
fn readable(
    name: &str,
    method: &Method,
    path: &str,
    query: Option<&str>,
    namespaces: &BTreeSet<String>,
) -> bool {
    if !crate::proxy::allowed(method, path, query, namespaces) {
        return false;
    }
    match name.strip_prefix("reader/") {
        Some(namespace) => {
            crate::proxy::allowed(method, path, query, &BTreeSet::from([namespace.to_owned()]))
        }
        None => name == "reader",
    }
}
pub fn router(namespaces: BTreeSet<String>, read: PathBuf, write: PathBuf) -> Result<Router> {
    anyhow::ensure!(
        kimage_core::scope::valid_registry_namespaces(&namespaces),
        "InvalidNamespaceAllowlist"
    );
    let gate = Gate {
        client: reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(120))
            .build()?,
        namespaces,
        read,
        write,
        permits: Arc::new(tokio::sync::Semaphore::new(16)),
    };
    Ok(Router::new()
        .fallback(forward)
        .layer(tower_http::limit::RequestBodyLimitLayer::new(
            512 * 1024 * 1024,
        ))
        .with_state(Arc::new(gate)))
}
fn writable(
    method: &Method,
    path: &str,
    query: Option<&str>,
    namespaces: &BTreeSet<String>,
) -> bool {
    if crate::proxy::allowed(method, path, query, namespaces) {
        return true;
    }
    let Some(tail) = path.strip_prefix("/v2/") else {
        return false;
    };
    let parts: Vec<_> = tail.split('/').collect();
    if parts.len() < 4
        || !kimage_core::scope::registry_allows(namespaces, parts[0])
        || !crate::proxy::valid_identity(parts[1])
    {
        return false;
    }
    if parts.len() == 4
        && parts[2] == "manifests"
        && kimage_core::oci::valid_digest(parts[3])
        && *method == Method::PUT
        && query.is_none()
    {
        return true;
    }
    if parts.len() == 5 && parts[2] == "blobs" && parts[3] == "uploads" {
        if parts[4].is_empty() {
            return *method == Method::POST && query.is_none();
        }
        return parts[4].len() <= 128
            && parts[4]
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            && matches!(
                *method,
                Method::PUT | Method::PATCH | Method::GET | Method::DELETE
            );
    }
    false
}
async fn forward(State(g): State<Arc<Gate>>, r: Request) -> Response {
    let auth = r
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Basic "))
        .and_then(|v| STANDARD.decode(v).ok())
        .and_then(|v| String::from_utf8(v).ok());
    let credentials = auth.as_ref().and_then(|v| v.split_once(':'));
    let authorized = if let Some((name, password)) = credentials {
        let file = match name {
            "reader" => Some(&g.read),
            "publisher" => Some(&g.write),
            _ if name.starts_with("reader/") => Some(&g.read),
            _ => None,
        };
        file.and_then(|p| std::fs::read_to_string(p).ok())
            .is_some_and(|expected| {
                !expected.trim_end().is_empty()
                    && credential_matches(name, password, expected.trim_end())
            })
    } else {
        false
    };
    if !authorized {
        return (
            StatusCode::UNAUTHORIZED,
            [("www-authenticate", "Basic realm=\"kimage\"")],
        )
            .into_response();
    }
    let publisher = credentials.unwrap().0 == "publisher";
    if !(if publisher {
        writable(r.method(), r.uri().path(), r.uri().query(), &g.namespaces)
    } else {
        readable(
            credentials.unwrap().0,
            r.method(),
            r.uri().path(),
            r.uri().query(),
            &g.namespaces,
        )
    }) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Ok(permit) = g.permits.clone().try_acquire_owned() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let (parts, body) = r.into_parts();
    let mut req = g
        .client
        .request(parts.method, format!("http://127.0.0.1:5000{}", parts.uri));
    for h in [
        "content-type",
        "content-length",
        "content-range",
        "accept",
        "range",
        "if-range",
        "if-none-match",
    ] {
        if let Some(v) = parts.headers.get(h) {
            req = req.header(h, v);
        }
    }
    let Ok(response) = req
        .body(reqwest::Body::wrap_stream(body.into_data_stream()))
        .send()
        .await
    else {
        return StatusCode::BAD_GATEWAY.into_response();
    };
    if response.status().is_redirection() {
        return StatusCode::BAD_GATEWAY.into_response();
    }
    let mut builder = Response::builder().status(response.status());
    for h in [
        "content-type",
        "content-length",
        "content-range",
        "accept-ranges",
        "docker-content-digest",
        "docker-distribution-api-version",
        "docker-upload-uuid",
        "range",
        "etag",
    ] {
        if let Some(v) = response.headers().get(h) {
            builder = builder.header(h, v);
        }
    }
    if let Some(v) = response.headers().get("location") {
        let relative = v
            .to_str()
            .ok()
            .and_then(|s| reqwest::Url::parse(s).ok())
            .filter(|u| u.host_str() == Some("127.0.0.1") && u.port() == Some(5000))
            .map(|u| {
                format!(
                    "{}{}",
                    u.path(),
                    u.query().map(|q| format!("?{q}")).unwrap_or_default()
                )
            });
        let Some(location) = relative else {
            return StatusCode::BAD_GATEWAY.into_response();
        };
        builder = builder.header("location", location);
    }
    let stream = response.bytes_stream().map(move |part| {
        let _ = &permit;
        part
    });
    builder
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn namespace_reader_http_requests_refuse_other_scopes_and_privileges() {
        let directory = tempfile::tempdir().unwrap();
        let read = directory.path().join("read");
        let write = directory.path().join("write");
        std::fs::write(&read, "read-system-password").unwrap();
        std::fs::write(&write, "write-system-password").unwrap();
        let app = router(BTreeSet::from(["apps".into(), "other".into()]), read, write).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let key = namespace_reader_password("read-system-password", "apps").unwrap();
        let own = format!(
            "/v2/apps/system-fixture/manifests/sha256:{}",
            "a".repeat(64)
        );
        let foreign = own.replace("/apps/", "/other/");
        for (method, name, password, path, expected) in [
            (
                Method::GET,
                "reader/apps",
                key.as_str(),
                foreign.as_str(),
                StatusCode::FORBIDDEN,
            ),
            (
                Method::PUT,
                "reader/apps",
                key.as_str(),
                own.as_str(),
                StatusCode::FORBIDDEN,
            ),
            (
                Method::GET,
                "reader/other",
                key.as_str(),
                foreign.as_str(),
                StatusCode::UNAUTHORIZED,
            ),
            (
                Method::GET,
                "reader",
                key.as_str(),
                own.as_str(),
                StatusCode::UNAUTHORIZED,
            ),
            (
                Method::PUT,
                "publisher",
                key.as_str(),
                own.as_str(),
                StatusCode::UNAUTHORIZED,
            ),
            (
                Method::GET,
                "reader/apps",
                "read-system-password",
                own.as_str(),
                StatusCode::UNAUTHORIZED,
            ),
        ] {
            let response = client
                .request(method, format!("http://{address}{path}"))
                .basic_auth(name, Some(password))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), expected);
        }
        server.abort();
    }
    #[test]
    fn derived_readers_cannot_change_namespaces_write_or_impersonate_system_readers() {
        let secret = "installation-read-password";
        let apps = namespace_reader_password(secret, "apps").unwrap();
        let other = namespace_reader_password(secret, "other").unwrap();
        assert_eq!(apps.len(), 64);
        assert_ne!(apps, other);
        assert_eq!(apps, namespace_reader_password(secret, "apps").unwrap());
        assert!(credential_matches("reader/apps", &apps, secret));
        assert!(!credential_matches("reader/other", &apps, secret));
        assert!(!credential_matches("reader", &apps, secret));
        assert!(!credential_matches("publisher", &apps, "publish-password"));
        assert!(!credential_matches("reader/apps", secret, secret));
        assert!(credential_matches("reader", secret, secret));
        assert!(credential_matches(
            "publisher",
            "publish-password",
            "publish-password"
        ));
        for namespace in ["", "../apps", "apps/other", "Apps", "a_b"] {
            assert!(namespace_reader_password(secret, namespace).is_err());
        }
        let namespaces = BTreeSet::from(["apps".into(), "other".into()]);
        let digest = format!("sha256:{}", "a".repeat(64));
        for kind in ["manifests", "blobs"] {
            let own = format!("/v2/apps/system-fixture/{kind}/{digest}");
            let foreign = own.replace("/apps/", "/other/");
            assert!(readable(
                "reader/apps",
                &Method::GET,
                &own,
                None,
                &namespaces
            ));
            assert!(readable(
                "reader/apps",
                &Method::HEAD,
                &own,
                None,
                &namespaces
            ));
            assert!(!readable(
                "reader/apps",
                &Method::GET,
                &foreign,
                None,
                &namespaces
            ));
            assert!(!readable(
                "reader/apps",
                &Method::PUT,
                &own,
                None,
                &namespaces
            ));
            assert!(!readable(
                "reader/apps",
                &Method::GET,
                &own,
                Some("tag=latest"),
                &namespaces
            ));
            assert!(readable(
                "reader",
                &Method::GET,
                &foreign,
                None,
                &namespaces
            ));
            assert!(!readable(
                "reader/apps",
                &Method::GET,
                &own,
                None,
                &BTreeSet::from(["other".into()])
            ));
        }
    }
    #[test]
    fn independent_write_scope_and_constant_time_credentials() {
        let ns = BTreeSet::from(["ns".into()]);
        assert!(writable(
            &Method::POST,
            "/v2/ns/system-registry/blobs/uploads/",
            None,
            &ns
        ));
        assert!(!writable(
            &Method::DELETE,
            "/v2/ns/system-registry/manifests/latest",
            None,
            &ns
        ));
        assert!(equals(b"long secret", b"long secret"));
        assert!(!equals(b"long secret", b"other"));
    }
}

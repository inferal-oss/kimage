//! Fixed-cardinality process-local metrics. No names, paths, recipes or secrets.
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};
static ACTIVE: AtomicU64 = AtomicU64::new(0);
static COMPLETE: AtomicU64 = AtomicU64::new(0);
static PERMANENT: AtomicU64 = AtomicU64::new(0);
static RETRY: AtomicU64 = AtomicU64::new(0);
static ALLOWED: AtomicU64 = AtomicU64::new(0);
static DENIED: AtomicU64 = AtomicU64::new(0);
static SCHEMA_PENDING: AtomicU64 = AtomicU64::new(0);
static WAITING: AtomicU64 = AtomicU64::new(0);
static READY_IMAGES: AtomicU64 = AtomicU64::new(0);
static PENDING_IMAGES: AtomicU64 = AtomicU64::new(0);
static READY_RELEASES: AtomicU64 = AtomicU64::new(0);
static READY_NODES: AtomicU64 = AtomicU64::new(0);
static CATALOG_TIME: AtomicU64 = AtomicU64::new(0);
static CATALOG_COMPLETE: AtomicU64 = AtomicU64::new(0);
static NODE_TIME: AtomicU64 = AtomicU64::new(0);
struct Measurements {
    count: AtomicU64,
    micros: AtomicU64,
    failures: AtomicU64,
    bytes: AtomicU64,
    active: AtomicU64,
}
impl Measurements {
    const fn new() -> Self {
        Self {
            count: AtomicU64::new(0),
            micros: AtomicU64::new(0),
            failures: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            active: AtomicU64::new(0),
        }
    }
}
static BUILD: Measurements = Measurements::new();
static PUBLICATION: Measurements = Measurements::new();
static PULL: Measurements = Measurements::new();
#[derive(Clone, Copy)]
pub enum Stage {
    Build,
    Publication,
    Pull,
}
impl Stage {
    fn measurements(self) -> &'static Measurements {
        match self {
            Self::Build => &BUILD,
            Self::Publication => &PUBLICATION,
            Self::Pull => &PULL,
        }
    }
}
fn add(counter: &AtomicU64, value: u64) {
    let _ = counter.try_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
        Some(n.saturating_add(value))
    });
}
pub fn transferred(stage: Stage, count: usize) {
    add(&stage.measurements().bytes, count as u64);
}
/// Includes cancellation and body-stream abandonment in failures/latency.
pub struct Observation {
    stage: Stage,
    started: Instant,
    success: bool,
}
impl Observation {
    pub fn begin(stage: Stage) -> Self {
        increment(&stage.measurements().active);
        Self {
            stage,
            started: Instant::now(),
            success: false,
        }
    }
    pub fn success(&mut self) {
        self.success = true;
    }
    pub fn bytes(&self, count: usize) {
        transferred(self.stage, count);
    }
}
impl Drop for Observation {
    fn drop(&mut self) {
        let values = self.stage.measurements();
        increment(&values.count);
        add(
            &values.micros,
            self.started.elapsed().as_micros().min(u64::MAX as u128) as u64,
        );
        if !self.success {
            increment(&values.failures);
        }
        values.active.fetch_sub(1, Ordering::Relaxed);
    }
}
/// HTTP/1 may drop a body as soon as its declared Content-Length is written,
/// without polling EOF. Account for complete fixed-length bodies at their last
/// byte; unknown-length bodies still require EOF. Short/error bodies fail.
pub fn observe_stream<S, E>(
    stream: S,
    mut measured: Observation,
    expected: Option<u64>,
    successful: bool,
) -> impl futures::Stream<Item = Result<axum::body::Bytes, E>>
where
    S: futures::Stream<Item = Result<axum::body::Bytes, E>> + Unpin,
{
    use futures::StreamExt;
    if successful && expected == Some(0) {
        measured.success();
    }
    futures::stream::unfold(
        (stream, measured, successful, 0_u64),
        move |(mut stream, mut measured, mut successful, mut received)| async move {
            match stream.next().await {
                Some(part) => {
                    match &part {
                        Ok(bytes) => {
                            measured.bytes(bytes.len());
                            received = received.saturating_add(bytes.len() as u64);
                            measured.success = successful && expected == Some(received);
                        }
                        Err(_) => {
                            successful = false;
                            measured.success = false;
                        }
                    }
                    Some((part, (stream, measured, successful, received)))
                }
                None => {
                    if successful && expected.is_none_or(|size| size == received) {
                        measured.success();
                    }
                    None
                }
            }
        },
    )
}
pub struct Waiting;
impl Waiting {
    pub fn begin() -> Self {
        increment(&WAITING);
        Self
    }
}
impl Drop for Waiting {
    fn drop(&mut self) {
        WAITING.fetch_sub(1, Ordering::Relaxed);
    }
}
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
pub fn nodes_ready(count: usize) {
    READY_NODES.store(count as u64, Ordering::Relaxed);
    NODE_TIME.store(now(), Ordering::Relaxed);
}
/// Fixed-memory paginated sampling. A truncated/failed sample never replaces
/// complete counts; freshness/completeness gauges expose stale samples.
pub async fn sample_catalog(client: &kube::Client, namespace: &str) -> anyhow::Result<()> {
    use kube::{Api, api::ListParams};
    let images: Api<crate::api::Image> = Api::namespaced(client.clone(), namespace);
    let releases: Api<crate::api::ImageRelease> = Api::namespaced(client.clone(), namespace);
    CATALOG_COMPLETE.store(0, Ordering::Relaxed);
    let mut ready = 0;
    let mut pending = 0;
    let mut verified = 0;
    for kind in [false, true] {
        let mut token = String::new();
        for page in 0..100 {
            let params = ListParams::default().limit(100).continue_token(&token);
            token = if kind {
                let list = releases.list(&params).await?;
                verified += list
                    .items
                    .iter()
                    .filter(|v| {
                        v.metadata.deletion_timestamp.is_none()
                            && v.status.as_ref().is_some_and(|s| s.verified)
                    })
                    .count() as u64;
                list.metadata.continue_.unwrap_or_default()
            } else {
                let list = images.list(&params).await?;
                for image in list
                    .items
                    .iter()
                    .filter(|v| v.metadata.deletion_timestamp.is_none())
                {
                    if image.status.as_ref().is_some_and(|s| {
                        s.observed_generation == image.metadata.generation.unwrap_or(1)
                            && s.conditions
                                .iter()
                                .any(|c| c.type_ == "Ready" && c.status == "True")
                    }) {
                        ready += 1;
                    } else {
                        pending += 1;
                    }
                }
                list.metadata.continue_.unwrap_or_default()
            };
            if token.is_empty() {
                break;
            }
            anyhow::ensure!(page < 99, "MetricsCatalogSampleLimit");
        }
    }
    READY_IMAGES.store(ready, Ordering::Relaxed);
    PENDING_IMAGES.store(pending, Ordering::Relaxed);
    READY_RELEASES.store(verified, Ordering::Relaxed);
    CATALOG_TIME.store(now(), Ordering::Relaxed);
    CATALOG_COMPLETE.store(1, Ordering::Relaxed);
    Ok(())
}
fn increment(counter: &AtomicU64) {
    let _ = counter.try_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
        Some(n.saturating_add(1))
    });
}
pub struct Active;
impl Active {
    pub fn begin() -> Self {
        increment(&ACTIVE);
        Self
    }
}
impl Drop for Active {
    fn drop(&mut self) {
        ACTIVE.fetch_sub(1, Ordering::Relaxed);
    }
}
pub fn publication_complete() {
    increment(&COMPLETE);
}
pub fn failure(permanent: bool) {
    increment(if permanent { &PERMANENT } else { &RETRY });
}
pub fn admission(allowed: bool) {
    increment(if allowed { &ALLOWED } else { &DENIED });
}
pub fn schema_pending() {
    increment(&SCHEMA_PENDING);
}
pub async fn render() -> String {
    use std::fmt::Write;
    let mut output = format!(
        "# TYPE kimage_operations_active gauge\nkimage_operations_active {}\n# TYPE kimage_publication_verified_total counter\nkimage_publication_verified_total {}\n# TYPE kimage_reconcile_failures_total counter\nkimage_reconcile_failures_total{{kind=\"permanent\"}} {}\nkimage_reconcile_failures_total{{kind=\"retry\"}} {}\n# TYPE kimage_admission_requests_total counter\nkimage_admission_requests_total{{decision=\"allowed\"}} {}\nkimage_admission_requests_total{{decision=\"denied\"}} {}\n# TYPE kimage_schema_pending_total counter\nkimage_schema_pending_total {}\n",
        ACTIVE.load(Ordering::Relaxed),
        COMPLETE.load(Ordering::Relaxed),
        PERMANENT.load(Ordering::Relaxed),
        RETRY.load(Ordering::Relaxed),
        ALLOWED.load(Ordering::Relaxed),
        DENIED.load(Ordering::Relaxed),
        SCHEMA_PENDING.load(Ordering::Relaxed)
    );
    output.push_str("# TYPE kimage_stage_duration_seconds summary\n# TYPE kimage_stage_failures_total counter\n# TYPE kimage_stage_transferred_bytes_total counter\n# TYPE kimage_stage_active gauge\n");
    for (name, values) in [
        ("build", &BUILD),
        ("publication", &PUBLICATION),
        ("pull", &PULL),
    ] {
        let _ = writeln!(
            output,
            "kimage_stage_duration_seconds_count{{stage=\"{name}\"}} {}\nkimage_stage_duration_seconds_sum{{stage=\"{name}\"}} {:.6}\nkimage_stage_failures_total{{stage=\"{name}\"}} {}\nkimage_stage_transferred_bytes_total{{stage=\"{name}\"}} {}\nkimage_stage_active{{stage=\"{name}\"}} {}",
            values.count.load(Ordering::Relaxed),
            values.micros.load(Ordering::Relaxed) as f64 / 1_000_000.0,
            values.failures.load(Ordering::Relaxed),
            values.bytes.load(Ordering::Relaxed),
            values.active.load(Ordering::Relaxed)
        );
    }
    for (name, value) in [
        ("reconcile_waiting", &WAITING),
        ("images_ready", &READY_IMAGES),
        ("images_pending", &PENDING_IMAGES),
        ("releases_verified", &READY_RELEASES),
        ("nodes_eligible", &READY_NODES),
        ("catalog_sample_timestamp_seconds", &CATALOG_TIME),
        ("catalog_sample_complete", &CATALOG_COMPLETE),
        ("node_sample_timestamp_seconds", &NODE_TIME),
    ] {
        let _ = writeln!(
            output,
            "# TYPE kimage_{name} gauge\nkimage_{name} {}",
            value.load(Ordering::Relaxed)
        );
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn stream_completion_and_abandonment_account_without_identity_labels() {
        let count = BUILD.count.load(Ordering::Relaxed);
        let failures = BUILD.failures.load(Ordering::Relaxed);
        let bytes = BUILD.bytes.load(Ordering::Relaxed);
        {
            let mut measured = Observation::begin(Stage::Build);
            measured.bytes(17);
            measured.success();
        }
        {
            let _abandoned = Observation::begin(Stage::Build);
        }
        assert_eq!(BUILD.count.load(Ordering::Relaxed), count + 2);
        assert_eq!(BUILD.failures.load(Ordering::Relaxed), failures + 1);
        assert_eq!(BUILD.bytes.load(Ordering::Relaxed), bytes + 17);
        assert_eq!(BUILD.active.load(Ordering::Relaxed), 0);
        let text = render().await;
        assert!(text.contains("kimage_stage_duration_seconds_count{stage=\"pull\"}"));
        assert!(!text.contains("namespace="));
        assert!(!text.contains("digest="));
        assert!(text.len() < 16384);
    }
    #[tokio::test]
    async fn real_http_fixed_length_completion_and_truncation_have_distinct_outcomes() {
        use axum::{
            body::{Body, Bytes},
            response::Response,
        };
        let count = PULL.count.load(Ordering::Relaxed);
        let failures = PULL.failures.load(Ordering::Relaxed);
        let listener = crate::serving::listener("127.0.0.1:0".parse().unwrap()).unwrap();
        let address = listener.local_addr().unwrap();
        let router = axum::Router::new().fallback(|request: axum::extract::Request| async move {
            let data: &'static [u8] = if request.uri().path() == "/ok" {
                b"0123456789abcdefg"
            } else {
                b"short!!!"
            };
            let stream = futures::stream::iter([Ok::<_, std::io::Error>(Bytes::from_static(data))]);
            Response::builder()
                .header("content-length", "17")
                .body(Body::from_stream(observe_stream(
                    stream,
                    Observation::begin(Stage::Pull),
                    Some(17),
                    true,
                )))
                .unwrap()
        });
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(3))
            .build()
            .unwrap();
        assert_eq!(
            client
                .get(format!("http://{address}/ok"))
                .send()
                .await
                .unwrap()
                .bytes()
                .await
                .unwrap()
                .len(),
            17
        );
        let short = client.get(format!("http://{address}/short")).send().await;
        assert!(match short {
            Ok(response) => response.bytes().await.is_err(),
            Err(_) => true,
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while PULL.count.load(Ordering::Relaxed) < count + 2
                || PULL.failures.load(Ordering::Relaxed) < failures + 1
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(PULL.count.load(Ordering::Relaxed), count + 2);
        assert_eq!(PULL.failures.load(Ordering::Relaxed), failures + 1);
        server.abort();
    }
}

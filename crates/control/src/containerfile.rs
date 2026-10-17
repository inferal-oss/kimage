//! Pinned Containerfile/BuildKit transport adapter. Output stays in private diagnostics.
use crate::input_snapshot::Capture;
use anyhow::{Context, Result, ensure};
use kimage_core::build::{Build, CacheMode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path, process::Stdio, time::Duration};
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Session {
    #[serde(default = "kimage_core::oci::Platform::linux_amd64")]
    pub platform: kimage_core::oci::Platform,
    pub build: Build,
    pub endpoint: String,
    pub tls_server_name: String,
    pub worker_uid: String,
    pub snapshot_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub image_inputs: Vec<crate::image_inputs::FrozenImage>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub namespace: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub registry_origin: String,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SolveSummary {
    pub engine_worker_id: String,
    pub engine_operation_id: String,
    pub cached_run_vertices: u64,
    pub executed_run_vertices: u64,
    pub effective_cache_mode: CacheMode,
}
pub fn validate_definition(bytes: &[u8]) -> Result<()> {
    let text = std::str::from_utf8(bytes).context("DefinitionMustBeUtf8")?;
    ensure!(
        !text.is_empty()
            && text.len() <= kimage_core::build::DEFINITION_LIMIT
            && !text.contains('\0'),
        "InvalidDefinitionText"
    );
    for line in text.trim_start_matches('\u{feff}').lines() {
        let line = line.trim_start();
        if let Some(comment) = line.strip_prefix('#')
            && let Some((key, _)) = comment.split_once('=')
        {
            ensure!(
                !key.trim().eq_ignore_ascii_case("syntax"),
                "ExternalFrontendDirectiveDenied"
            );
        }
    }
    Ok(())
}
pub fn source_policy(inputs: &[crate::image_inputs::FrozenImage]) -> Value {
    // The pinned engine uses the LAST matching allow/deny rule. Local sessions
    // are the only source type exposed by this initial offline adapter.
    let mut policy = json!({"version":1,"rules":[{"action":"DENY","selector":{"identifier":"(?s)^.*$","match_type":"REGEX"}},{"action":"ALLOW","selector":{"identifier":"local://context","match_type":"EXACT"}},{"action":"ALLOW","selector":{"identifier":"local://dockerfile","match_type":"EXACT"}}]});
    for input in inputs {
        policy["rules"].as_array_mut().unwrap().push(json!({"action":"ALLOW","selector":{"identifier":format!("oci-layout://docker.io/library/{}@{}",input.name,input.root.digest),"match_type":"EXACT"}}));
    }
    policy
}
pub fn arguments(session: &Session, capture: &Capture) -> Result<Vec<String>> {
    ensure!(session.platform.supported(), "UnsupportedBuildPlatform");
    session.build.validate()?;
    crate::image_inputs::validate(&session.image_inputs, &session.build, &session.namespace)?;
    ensure!(
        session.build.execution.builder_ref == "default",
        "UnsupportedBuilder"
    );
    let legacy = session.platform == kimage_core::oci::Platform::linux_amd64()
        && session.endpoint == format!("tcp://{}:1234", session.tls_server_name);
    let canonical = session
        .tls_server_name
        .split_once('.')
        .is_some_and(|(service, namespace)| {
            service
                .strip_suffix("-builder")
                .is_some_and(kimage_core::selector::dns_label)
                && namespace
                    .strip_suffix(".svc")
                    .is_some_and(kimage_core::selector::dns_label)
        });
    let native = session
        .tls_server_name
        .split_once('.')
        .is_some_and(|(service, namespace)| {
            service
                .strip_suffix("-builder")
                .is_some_and(kimage_core::selector::dns_label)
                && namespace
                    .strip_suffix(".svc")
                    .is_some_and(kimage_core::selector::dns_label)
                && session.endpoint
                    == format!(
                        "tcp://{service}-{}.{namespace}:1234",
                        session.platform.architecture
                    )
        });
    ensure!(
        kimage_core::selector::dns_subdomain(&session.tls_server_name)
            && canonical
            && (legacy || native),
        "InvalidApprovedBuilderEndpoint"
    );
    let definition = if capture.definition_sensitive {
        "/sensitive/definition"
    } else {
        "/snapshot/definition"
    };
    let mut args = vec![
        "--addr".into(),
        session.endpoint.clone(),
        "--tlsservername".into(),
        session.tls_server_name.clone(),
        "--tlscacert".into(),
        "/builder-credentials/ca.crt".into(),
        "--tlscert".into(),
        "/builder-credentials/client.crt".into(),
        "--tlskey".into(),
        "/builder-credentials/client.key".into(),
        "build".into(),
        "--frontend".into(),
        "dockerfile.v0".into(),
        "--local".into(),
        "context=/snapshot/context".into(),
        "--local".into(),
        format!("dockerfile={definition}"),
        "--opt".into(),
        "filename=Containerfile".into(),
        "--opt".into(),
        format!("platform={}", session.platform.name()),
        "--opt".into(),
        "force-network-mode=none".into(),
        "--source-policy-file".into(),
        "/snapshot/source-policy.json".into(),
        "--ref-file".into(),
        "/snapshot/engine-reference".into(),
        "--output".into(),
        "type=oci,dest=/snapshot/result.oci.tar".into(),
        "--progress".into(),
        "rawjson".into(),
    ];
    for input in &session.image_inputs {
        args.extend([
            "--oci-layout".into(),
            format!("{}=/snapshot/image-inputs/{}", input.name, input.name),
            "--opt".into(),
            format!(
                "context:{}=oci-layout://{}@{}",
                input.name, input.name, input.root.digest
            ),
        ]);
    }
    if session.build.cache.mode == CacheMode::Disabled
        || session.build.cache.invalidation_token.is_some()
    {
        args.push("--no-cache".into());
    }
    for binding in &session.build.build_secrets {
        args.extend([
            "--secret".into(),
            format!("id={},src=/sensitive/secrets/{}", binding.id, binding.id),
        ]);
    }
    Ok(args)
}
#[derive(Default)]
struct Progress {
    vertices: BTreeMap<String, bool>,
}
impl Progress {
    fn observe(&mut self, line: &[u8]) -> Result<()> {
        let Ok(value) = serde_json::from_slice::<Value>(line) else {
            return Ok(());
        };
        if let Some(vertices) = value
            .get("Vertexes")
            .or_else(|| value.get("vertexes"))
            .and_then(Value::as_array)
        {
            for vertex in vertices {
                let Some(name) = vertex
                    .get("Name")
                    .or_else(|| vertex.get("name"))
                    .and_then(Value::as_str)
                else {
                    continue;
                };
                if !name.contains("] RUN ") {
                    continue;
                }
                let Some(digest) = vertex
                    .get("Digest")
                    .or_else(|| vertex.get("digest"))
                    .and_then(Value::as_str)
                else {
                    continue;
                };
                ensure!(
                    kimage_core::oci::valid_digest(digest),
                    "InvalidBuilderProgressDigest"
                );
                let cached = vertex
                    .get("Cached")
                    .or_else(|| vertex.get("cached"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let previous = self.vertices.entry(digest.into()).or_default();
                *previous |= cached;
                ensure!(self.vertices.len() <= 8192, "BuildProgressLimit");
            }
        }
        Ok(())
    }
}
const PROGRESS_RECORD_LIMIT: usize = 1024 * 1024;

fn progress_chunk(
    bytes: &[u8],
    line: &mut Vec<u8>,
    progress: &mut Progress,
    log: &mut crate::build_logs::Buffer,
) -> Result<()> {
    for &byte in bytes {
        if byte == b'\n' {
            log.progress(line);
            progress.observe(line)?;
            line.clear();
        } else {
            ensure!(line.len() < PROGRESS_RECORD_LIMIT, "BuildProgressLineLimit");
            line.push(byte);
        }
    }
    Ok(())
}

pub async fn solve(
    session: &Session,
    capture: &Capture,
    cancel: &CancellationToken,
) -> Result<SolveSummary> {
    let engine_worker_id = worker_identity(session, capture).await?;
    let definition = if capture.definition_sensitive {
        "/sensitive/definition/Containerfile"
    } else {
        "/snapshot/definition/Containerfile"
    };
    validate_definition(&std::fs::read(definition)?)?;
    std::fs::write(
        "/snapshot/source-policy.json",
        serde_json::to_vec(&source_policy(&session.image_inputs))?,
    )?;
    let mut child = tokio::process::Command::new("/buildctl")
        .args(arguments(session, capture)?)
        .env_clear()
        .env("PATH", "/")
        .env("HOME", "/nonexistent")
        .env("OTEL_TRACES_EXPORTER", "none")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("BuilderClientUnavailable")?;
    let mut stderr = child.stderr.take().context("BuilderProgressUnavailable")?;
    let drain = async {
        let mut progress = Progress::default();
        let mut chunk = [0_u8; 16384];
        let mut line = Vec::new();
        let mut total = 0_usize;
        let mut log = crate::build_logs::Buffer::default();
        log.persist()?;
        loop {
            let n = stderr.read(&mut chunk).await?;
            if n == 0 {
                break;
            }
            total += n;
            ensure!(total <= 16 * 1024 * 1024, "BuildProgressLimit");
            progress_chunk(&chunk[..n], &mut line, &mut progress, &mut log)?;
            log.persist()?;
        }
        if !line.is_empty() {
            log.progress(&line);
            log.persist()?;
            progress.observe(&line)?;
        }
        Ok::<_, anyhow::Error>(progress)
    };
    let monitor = async {
        loop {
            if let Ok(metadata) = tokio::fs::metadata("/snapshot/result.oci.tar").await {
                ensure!(metadata.len() <= 1024 * 1024 * 1024, "BuildExportTooLarge");
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        #[allow(unreachable_code)]
        Ok::<(), anyhow::Error>(())
    };
    let result = {
        let work = async {
            let wait = async { Ok::<_, anyhow::Error>(child.wait().await?) };
            let (status, progress) = tokio::try_join!(wait, drain)?;
            ensure!(status.success(), "ContainerfileSolveFailed");
            Ok::<_, anyhow::Error>(progress)
        };
        tokio::select! {
            _=cancel.cancelled()=>Err(anyhow::anyhow!("OperationCancelled")),
            result=monitor=>result.and(Err(anyhow::anyhow!("BuildMonitorStopped"))),
            result=tokio::time::timeout(Duration::from_secs(600),work)=>match result{Ok(value)=>value,Err(_)=>Err(anyhow::anyhow!("BuildDeadlineExceeded"))},
        }
    };
    if result.is_err() {
        let _ = child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(10), child.wait()).await;
    }
    let progress = result?;
    ensure!(
        Path::new("/snapshot/result.oci.tar").is_file(),
        "MissingBuildExport"
    );
    let cached = progress.vertices.values().filter(|&&cached| cached).count() as u64;
    Ok(SolveSummary {
        engine_worker_id,
        engine_operation_id: {
            let id = std::fs::read_to_string("/snapshot/engine-reference")?;
            ensure!(
                !id.is_empty()
                    && id.len() <= 128
                    && id
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c)),
                "InvalidEngineOperationId"
            );
            id
        },
        cached_run_vertices: cached,
        executed_run_vertices: progress.vertices.len() as u64 - cached,
        effective_cache_mode: if session.build.cache.mode == CacheMode::Disabled
            || session.build.cache.invalidation_token.is_some()
        {
            CacheMode::Disabled
        } else {
            CacheMode::Local
        },
    })
}
async fn worker_identity(session: &Session, capture: &Capture) -> Result<String> {
    let args = arguments(session, capture)?;
    let mut child = tokio::process::Command::new("/buildctl")
        .args(&args[..10])
        .args(["debug", "workers", "--format", "{{json .}}"])
        .env_clear()
        .env("PATH", "/")
        .env("HOME", "/nonexistent")
        .env("OTEL_TRACES_EXPORTER", "none")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("BuilderClientUnavailable")?;
    let mut output = child.stdout.take().context("BuilderInfoUnavailable")?;
    let mut bytes = Vec::new();
    let result = tokio::time::timeout(Duration::from_secs(15), async {
        (&mut output).take(65537).read_to_end(&mut bytes).await?;
        ensure!(bytes.len() <= 65536, "BuilderInfoTooLarge");
        ensure!(child.wait().await?.success(), "BuilderUnavailable");
        Ok::<(), anyhow::Error>(())
    })
    .await;
    ensure!(result.is_ok(), "BuilderUnavailable");
    result??;
    let value: Value = serde_json::from_slice(&bytes).context("InvalidBuilderInfo")?;
    let worker = if let Some(list) = value.as_array() {
        ensure!(list.len() == 1, "UnsupportedWorkerCount");
        &list[0]
    } else {
        &value
    };
    let id = worker
        .get("ID")
        .or_else(|| worker.get("id"))
        .and_then(Value::as_str)
        .context("MissingEngineWorkerId")?;
    ensure!(
        !id.is_empty()
            && id.len() <= 128
            && id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c)),
        "InvalidEngineWorkerId"
    );
    let platforms = worker
        .get("Platforms")
        .or_else(|| worker.get("platforms"))
        .and_then(Value::as_array)
        .context("MissingWorkerPlatforms")?;
    ensure!(
        platforms.iter().any(
            |p| p.get("os").or_else(|| p.get("OS")).and_then(Value::as_str) == Some("linux")
                && p.get("architecture")
                    .or_else(|| p.get("Architecture"))
                    .and_then(Value::as_str)
                    == Some(session.platform.architecture.as_str())
        ),
        "UnsupportedWorkerPlatform"
    );
    Ok(id.into())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn batched_engine_logs_larger_than_256_kib_keep_the_final_bounded_tail() {
        use base64::{Engine, engine::general_purpose::STANDARD};
        let output = (0..5000)
            .map(|n| format!("kimage-diagnostic-output-{n:04}\n"))
            .collect::<String>();
        let record = json!({"vertexes":[{"name":"[stage 1/1] RUN /probe diagnose","digest":kimage_core::oci::digest(b"native-run"),"cached":false,"error":"engine-context ".repeat(18000)}],"logs":[{"data":STANDARD.encode(output)}]});
        let mut bytes = serde_json::to_vec(&record).unwrap();
        assert!(bytes.len() > 256 * 1024);
        assert!(bytes.len() < PROGRESS_RECORD_LIMIT);
        bytes.push(b'\n');
        let (mut line, mut progress, mut log) = (
            Vec::new(),
            Progress::default(),
            crate::build_logs::Buffer::default(),
        );
        for chunk in bytes.chunks(16384) {
            progress_chunk(chunk, &mut line, &mut progress, &mut log).unwrap();
        }
        assert!(line.is_empty());
        assert_eq!(progress.vertices.len(), 1);
        assert!(
            log.truncated
                && log.text.len() <= crate::build_logs::LIMIT
                && log.text.contains("kimage-diagnostic-output-4999")
        );
        let oversized = vec![b'x'; PROGRESS_RECORD_LIMIT + 1];
        assert_eq!(
            progress_chunk(&oversized, &mut line, &mut progress, &mut log)
                .unwrap_err()
                .to_string(),
            "BuildProgressLineLimit"
        );
    }
    #[test]
    fn native_services_share_only_the_canonical_tls_identity_and_bind_the_requested_platform() {
        let build:Build = serde_json::from_value(json!({"format":"Containerfile","definition":{"inline":"FROM scratch\nCOPY message /message\n"},"execution":{"strategy":"Direct","builderRef":"default"},"inputs":{"contextPath":"/workspace/context","volumes":[{"name":"source","configMap":{"name":"source"}}],"volumeMounts":[{"name":"source","mountPath":"/workspace/context","readOnly":true}]}})).unwrap();
        let capture = Capture {
            ordinary_digest: kimage_core::oci::digest(b"input"),
            definition_sensitive: false,
            definition_digest: kimage_core::oci::digest(b"definition"),
            files: 1,
            bytes: 1,
            image_inputs: Vec::new(),
        };
        for architecture in ["amd64", "arm64"] {
            let mut session = Session {
                platform: kimage_core::oci::Platform::linux(architecture),
                build: build.clone(),
                endpoint: format!("tcp://dev-builder-{architecture}.system.svc:1234"),
                tls_server_name: "dev-builder.system.svc".into(),
                worker_uid: "worker".into(),
                snapshot_id: "a".repeat(64),
                image_inputs: Vec::new(),
                namespace: String::new(),
                registry_origin: String::new(),
            };
            let args = arguments(&session, &capture).unwrap();
            assert_eq!(args[1], session.endpoint);
            assert_eq!(args[3], session.tls_server_name);
            assert!(args.contains(&format!("platform=linux/{architecture}")));
            session.endpoint = "tcp://external.invalid:1234".into();
            assert!(arguments(&session, &capture).is_err());
            let other = if architecture == "amd64" {
                "arm64"
            } else {
                "amd64"
            };
            session.endpoint = format!("tcp://dev-builder-{other}.system.svc:1234");
            assert!(arguments(&session, &capture).is_err());
            session.endpoint = "tcp://dev-builder.system.svc:1234".into();
            assert_eq!(
                arguments(&session, &capture).is_ok(),
                architecture == "amd64"
            );
        }
    }

    #[test]
    fn directives_cannot_select_remote_frontends_and_progress_never_parses_log_payloads() {
        assert!(validate_definition(b"# syntax=docker/dockerfile:latest\nFROM scratch\n").is_err());
        assert!(validate_definition(b"FROM scratch\nCOPY x /x\n").is_ok());
        let mut progress = Progress::default();
        let digest = kimage_core::oci::digest(b"step");
        progress.observe(&serde_json::to_vec(&json!({"Logs":[{"Data":{"Vertexes":[{"Name":"[stage 1/1] RUN forged","Digest":digest,"Cached":true}]}}]})).unwrap()).unwrap();
        assert!(progress.vertices.is_empty());
        progress.observe(&serde_json::to_vec(&json!({"Vertexes":[{"Name":"[stage 1/1] RUN private recipe","Digest":digest,"Cached":true}]})).unwrap()).unwrap();
        assert_eq!(progress.vertices.len(), 1);
        assert!(progress.vertices.values().all(|cached| *cached));
    }
}

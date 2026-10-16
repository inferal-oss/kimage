//! Private, bounded attempt diagnostics. Build output is never release metadata.
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use k8s_openapi::{
    ByteString,
    api::core::v1::{ConfigMap, Pod, Secret},
};
use kube::{
    Api, Client, ResourceExt,
    api::{AttachParams, DeleteParams, ListParams, PostParams, Preconditions},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, io::Write, time::Duration};
use tokio::io::AsyncReadExt;

pub const LIMIT: usize = 64 * 1024;
const RETAIN: usize = 5;
const LABEL: &str = "kimage.dev/build-log";
const PATH: &str = "/snapshot/build-log.json";

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Buffer {
    pub end: u64,
    pub text: String,
    pub truncated: bool,
}
impl Buffer {
    pub fn append(&mut self, text: &str) {
        // Strip terminal control sequences' introducers, including OSC and C1.
        let clean: String = text
            .chars()
            .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
            .collect();
        self.end = self.end.saturating_add(clean.len() as u64);
        self.text.push_str(&clean);
        if self.text.len() > LIMIT {
            let mut start = self.text.len() - LIMIT;
            while !self.text.is_char_boundary(start) {
                start += 1;
            }
            self.text.drain(..start);
            self.truncated = true;
        }
    }
    pub fn progress(&mut self, line: &[u8]) {
        let Ok(value) = serde_json::from_slice::<Value>(line) else {
            self.append(&String::from_utf8_lossy(line));
            self.append("\n");
            return;
        };
        // Never recursively interpret user output as progress or diagnostic metadata.
        for vertex in value
            .get("vertexes")
            .or_else(|| value.get("Vertexes"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            for key in ["name", "error"] {
                let upper = if key == "name" { "Name" } else { "Error" };
                if let Some(text) = vertex
                    .get(key)
                    .or_else(|| vertex.get(upper))
                    .and_then(Value::as_str)
                {
                    self.append(text);
                    self.append("\n");
                }
            }
        }
        for log in value
            .get("logs")
            .or_else(|| value.get("Logs"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(data) = log
                .get("data")
                .or_else(|| log.get("Data"))
                .and_then(Value::as_str)
                && let Ok(bytes) = STANDARD.decode(data)
            {
                self.append(&String::from_utf8_lossy(&bytes));
            }
        }
        if let Some(error) = value.get("error").and_then(Value::as_str) {
            self.append(error);
            self.append("\n");
        }
    }
    pub fn persist(&self) -> Result<()> {
        std::fs::write(format!("{PATH}.pending"), serde_json::to_vec(self)?)?;
        std::fs::rename(format!("{PATH}.pending"), PATH)?;
        Ok(())
    }
    fn validate(&self) -> Result<()> {
        ensure!(
            self.text.len() <= LIMIT && self.end >= self.text.len() as u64,
            "InvalidBuildLog"
        );
        ensure!(
            self.text
                .chars()
                .all(|c| !c.is_control() || matches!(c, '\n' | '\t')),
            "InvalidBuildLog"
        );
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Snapshot {
    pub phase: String,
    pub log: Buffer,
}
pub fn snapshot() -> Result<Snapshot> {
    let log = match std::fs::read(PATH) {
        Ok(bytes) => {
            ensure!(bytes.len() <= 512 * 1024, "BuildLogTooLarge");
            serde_json::from_slice(&bytes)?
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Buffer::default(),
        Err(e) => return Err(e.into()),
    };
    log.validate()?;
    Ok(Snapshot {
        phase: crate::build_session::inspect()?["phase"]
            .as_str()
            .context("InvalidBuildPhase")?
            .into(),
        log,
    })
}
async fn live(client: Client, namespace: &str, pod: &str) -> Result<Snapshot> {
    tokio::time::timeout(Duration::from_secs(10), async {
        let pods: Api<Pod> = Api::namespaced(client, namespace);
        let mut process = pods
            .exec(
                pod,
                ["/snapshot/session", "build-logs"],
                &AttachParams::default().container("client").stderr(false),
            )
            .await?;
        let status = process.take_status().context("MissingBuildClientStatus")?;
        let mut bytes = Vec::new();
        let mut output = process.stdout().context("MissingBuildClientOutput")?;
        (&mut output)
            .take(512 * 1024 + 1)
            .read_to_end(&mut bytes)
            .await?;
        ensure!(bytes.len() <= 512 * 1024, "BuildLogTooLarge");
        drop(output);
        process.join().await?;
        ensure!(
            status
                .await
                .context("MissingBuildClientStatus")?
                .status
                .as_deref()
                == Some("Success"),
            "BuildLogUnavailable"
        );
        let result: Snapshot = serde_json::from_slice(&bytes)?;
        result.log.validate()?;
        ensure!(
            result.phase.len() <= 128
                && result
                    .phase
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b':'),
            "InvalidBuildPhase"
        );
        Ok(result)
    })
    .await
    .context("BuildLogDeadline")?
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Attempt {
    pub schema: u32,
    pub image_uid: String,
    pub image: String,
    pub version: String,
    pub generation: i64,
    pub platform: kimage_core::oci::Platform,
    pub snapshot_id: String,
    pub diagnostic: Snapshot,
}
fn name(id: &str) -> Result<String> {
    crate::direct::client_name(id)?;
    Ok(format!("buildlog-{id}"))
}
/// Called before client cleanup. Unavailable workers must not prevent fencing.
pub async fn archive(client: Client, namespace: &str, pod: &Pod, id: &str) -> Result<()> {
    let owner = pod
        .owner_references()
        .iter()
        .find(|o| o.kind == "Image" && Some(&o.uid) == pod.labels().get("kimage.dev/image-uid"))
        .context("BuildLogOwnershipConflict")?;
    let images: Api<crate::api::Image> = Api::namespaced(client.clone(), namespace);
    let image = images.get(&owner.name).await?;
    ensure!(
        image.uid().as_deref() == Some(&owner.uid),
        "BuildLogOwnershipConflict"
    );
    let owner = if let Some(parent_uid) = image.labels().get("kimage.dev/platform-parent") {
        let parent = image
            .owner_references()
            .iter()
            .find(|o| o.kind == "Image" && &o.uid == parent_uid)
            .context("BuildLogOwnershipConflict")?;
        let parent_image = images.get(&parent.name).await?;
        ensure!(
            parent_image.uid().as_deref() == Some(parent_uid),
            "BuildLogOwnershipConflict"
        );
        parent.clone()
    } else {
        owner.clone()
    };
    let secrets: Api<Secret> = Api::namespaced(client.clone(), namespace);
    let key = name(id)?;
    if let Some(existing) = secrets.get_opt(&key).await? {
        let archived = validate_archive(&existing, &owner.uid)?;
        return prune(&secrets, &owner.uid, &archived.platform).await;
    }
    let maps: Api<ConfigMap> = Api::namespaced(client.clone(), namespace);
    let map = maps.get(&format!("build-{}-session", &id[..20])).await?;
    ensure!(
        map.immutable == Some(true) && map.owner_references() == pod.owner_references(),
        "BuildLogOwnershipConflict"
    );
    let record: crate::direct_client::Record = serde_json::from_str(
        map.data
            .as_ref()
            .and_then(|d| d.get("record.json"))
            .context("MissingBuildSessionRecord")?,
    )?;
    ensure!(record.session.snapshot_id == id, "BuildLogIdentityConflict");
    let mut diagnostic = live(client, namespace, &pod.name_any())
        .await
        .unwrap_or_else(|_| Snapshot {
            phase: "Unavailable".into(),
            log: Buffer::default(),
        });
    if matches!(
        diagnostic.phase.as_str(),
        "Capturing" | "Captured" | "Submitted"
    ) {
        diagnostic.phase = "Interrupted".into();
    }
    let attempt = Attempt {
        schema: 1,
        image_uid: owner.uid.clone(),
        image: owner.name.clone(),
        version: pod
            .annotations()
            .get("kimage.dev/build-version")
            .cloned()
            .unwrap_or_default(),
        generation: record.generation,
        platform: record.session.platform,
        snapshot_id: id.into(),
        diagnostic,
    };
    let mut secret: Secret = serde_json::from_value(
        json!({"apiVersion":"v1","kind":"Secret","metadata":{"name":key,"labels":{LABEL:owner.uid,"kimage.dev/build-state":owner.uid,"kimage.dev/log-platform":attempt.platform.architecture},"ownerReferences":[owner]},"type":"Opaque","immutable":true}),
    )?;
    secret.data = Some(BTreeMap::from([(
        "attempt.json".into(),
        ByteString(serde_json::to_vec(&attempt)?),
    )]));
    match secrets.create(&PostParams::default(), &secret).await {
        Ok(_) => {}
        Err(kube::Error::Api(e)) if e.code == 409 => {
            validate_archive(&secrets.get(&key).await?, &owner.uid)?;
        }
        Err(e) => return Err(e.into()),
    }
    prune(&secrets, &owner.uid, &attempt.platform).await
}
async fn prune(
    secrets: &Api<Secret>,
    uid: &str,
    platform: &kimage_core::oci::Platform,
) -> Result<()> {
    let page = secrets
        .list(
            &ListParams::default()
                .labels(&format!(
                    "{LABEL}={},kimage.dev/log-platform={}",
                    uid, platform.architecture
                ))
                .limit(128),
        )
        .await?;
    ensure!(
        page.metadata
            .continue_
            .as_ref()
            .is_none_or(|v| v.is_empty()),
        "TooManyBuildLogs"
    );
    let mut items = page.items;
    items.sort_by_key(|s| (s.metadata.creation_timestamp.clone(), s.name_any()));
    let remove = items.len().saturating_sub(RETAIN);
    for item in items.into_iter().take(remove) {
        validate_archive(&item, uid)?;
        match secrets
            .delete(
                &item.name_any(),
                &DeleteParams {
                    preconditions: Some(Preconditions {
                        uid: item.uid(),
                        resource_version: item.resource_version(),
                    }),
                    ..Default::default()
                },
            )
            .await
        {
            Ok(_) => {}
            Err(kube::Error::Api(e)) if e.code == 404 || e.code == 409 => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
fn validate_archive(secret: &Secret, uid: &str) -> Result<Attempt> {
    ensure!(
        secret.immutable == Some(true)
            && secret.labels().get(LABEL).is_some_and(|v| v == uid)
            && secret
                .labels()
                .get("kimage.dev/build-state")
                .is_some_and(|v| v == uid)
            && secret
                .owner_references()
                .iter()
                .any(|o| o.kind == "Image" && o.uid == uid),
        "BuildLogOwnershipConflict"
    );
    let bytes = &secret
        .data
        .as_ref()
        .and_then(|d| d.get("attempt.json"))
        .context("MissingBuildLog")?
        .0;
    ensure!(bytes.len() <= 512 * 1024, "BuildLogTooLarge");
    let attempt: Attempt = serde_json::from_slice(bytes)?;
    ensure!(
        attempt.schema == 1
            && attempt.image_uid == uid
            && secret.name_any() == name(&attempt.snapshot_id)?
            && attempt.platform.supported(),
        "BuildLogIdentityConflict"
    );
    attempt.diagnostic.log.validate()?;
    Ok(attempt)
}
pub async fn read(
    client: Client,
    namespace: &str,
    image: &str,
    platform: Option<&kimage_core::oci::Platform>,
    attempt: Option<&str>,
    follow: bool,
    list: bool,
) -> Result<()> {
    let images: Api<crate::api::Image> = Api::namespaced(client.clone(), namespace);
    let image = images.get(image).await?;
    let uid = image.uid().context("MissingImageUid")?;
    let secrets: Api<Secret> = Api::namespaced(client.clone(), namespace);
    let pods: Api<Pod> = Api::namespaced(client.clone(), namespace);
    if let Some(id) = attempt {
        name(id)?;
    }
    if let Some(platform) = platform {
        ensure!(platform.supported(), "ExplicitBuildLogPlatformRequired");
    }
    let mut selected = attempt.map(str::to_owned);
    let mut offset = 0;
    let startup_deadline = tokio::time::Instant::now() + Duration::from_secs(300);
    let mut waiting_reported = false;
    loop {
        let mut variants = Vec::new();
        if image.spec.platform.is_automatic() {
            for platform in [
                kimage_core::oci::Platform::linux_amd64(),
                kimage_core::oci::Platform::linux_arm64(),
            ] {
                if let Some(child) = images
                    .get_opt(&crate::platform_build::child_name(&image, &platform)?)
                    .await?
                {
                    ensure!(
                        child
                            .owner_references()
                            .iter()
                            .any(|o| o.kind == "Image" && o.uid == uid),
                        "BuildLogOwnershipConflict"
                    );
                    variants.push(json!({"name":child.name_any(),"platform":child.spec.platform.name(),"version":child.spec.version,"conditions":child.status.as_ref().map(|s|&s.conditions)}));
                }
            }
        }
        let page = secrets
            .list(
                &ListParams::default()
                    .labels(&format!("{LABEL}={uid}"))
                    .limit(128),
            )
            .await?;
        ensure!(
            page.metadata
                .continue_
                .as_ref()
                .is_none_or(|v| v.is_empty()),
            "TooManyBuildLogs"
        );
        let mut archived = Vec::new();
        for secret in page {
            let a = validate_archive(&secret, &uid)?;
            if platform.is_none_or(|p| *p == a.platform) {
                archived.push((secret.metadata.creation_timestamp.clone(), a));
            }
        }
        let page = pods
            .list(
                &ListParams::default()
                    .labels(&format!(
                        "kimage.dev/build-log-image={uid},kimage.dev/role=builder-client"
                    ))
                    .limit(128),
            )
            .await?;
        ensure!(
            page.metadata
                .continue_
                .as_ref()
                .is_none_or(|v| v.is_empty()),
            "TooManyBuildClients"
        );
        let mut active = Vec::new();
        for pod in page {
            let owner = pod
                .owner_references()
                .iter()
                .find(|o| o.kind == "Image")
                .context("BuildLogOwnershipConflict")?;
            let producer = images.get(&owner.name).await?;
            ensure!(
                producer.uid().as_deref() == Some(&owner.uid)
                    && (owner.uid == uid
                        || producer.labels().get("kimage.dev/platform-parent") == Some(&uid)
                            && producer
                                .owner_references()
                                .iter()
                                .any(|o| o.kind == "Image" && o.uid == uid)),
                "BuildLogOwnershipConflict"
            );
            if platform.is_some_and(|p| {
                pod.annotations()
                    .get("kimage.dev/build-platform")
                    .is_none_or(|s| s != &p.name())
            }) {
                continue;
            }
            if let Some(id) = pod.annotations().get("kimage.dev/build-snapshot-id") {
                crate::direct::client_name(id)?;
                active.push((pod.metadata.creation_timestamp.clone(), id.clone(), pod));
            }
        }
        if list {
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &json!({"image":image.name_any(),"variants":variants,"conditions":image.status.as_ref().map(|s|&s.conditions),"retained":archived.iter().map(|(_,a)|json!({"attempt":a.snapshot_id,"version":a.version,"generation":a.generation,"platform":a.platform.name(),"phase":a.diagnostic.phase,"truncated":a.diagnostic.log.truncated,"bytes":a.diagnostic.log.end})).collect::<Vec<_>>(),"active":active.iter().map(|(_,id,p)|json!({"attempt":id,"platform":p.annotations().get("kimage.dev/build-platform")})).collect::<Vec<_>>()})
                )?
            );
            return Ok(());
        }
        if selected.is_none() {
            active.sort_by_key(|(time, id, _)| (time.clone(), id.clone()));
            archived.sort_by_key(|(time, a)| (time.clone(), a.snapshot_id.clone()));
            selected = active
                .last()
                .map(|(_, id, _)| id.clone())
                .or_else(|| archived.last().map(|(_, a)| a.snapshot_id.clone()));
        }
        let id = selected
            .as_deref()
            .context("NoBuildLogs: use kimage builds IMAGE to inspect build conditions")?;
        let snapshot = if let Some((_, _, pod)) = active.iter().find(|(_, other, _)| other == id) {
            // A durable attempt exists before its init containers finish. Follow
            // that same attempt through startup instead of failing an early exec.
            if follow && waiting_for_client(pod) {
                ensure!(
                    tokio::time::Instant::now() < startup_deadline,
                    "BuildClientStartupPending: inspect Pod events and kimage builds IMAGE"
                );
                if !waiting_reported {
                    eprintln!("attempt {id}: waiting for the build client");
                    waiting_reported = true;
                }
                tokio::select! {_=tokio::signal::ctrl_c()=>return Ok(()),_=tokio::time::sleep(Duration::from_secs(1))=>{}}
                continue;
            }
            match live(client.clone(), namespace, &pod.name_any()).await {
                Ok(s) => s,
                Err(e) => {
                    if let Some((_, a)) = archived.iter().find(|(_, a)| a.snapshot_id == id) {
                        a.diagnostic.clone()
                    } else if let Some(secret) = secrets.get_opt(&name(id)?).await? {
                        validate_archive(&secret, &uid)?.diagnostic
                    } else {
                        return Err(e);
                    }
                }
            }
        } else {
            archived
                .iter()
                .find(|(_, a)| a.snapshot_id == id)
                .context("BuildLogNotRetained")?
                .1
                .diagnostic
                .clone()
        };
        let (gap, text) = delta(&snapshot.log, offset)?;
        let mut output = std::io::stdout().lock();
        if gap {
            output.write_all(b"[earlier output truncated]\n")?;
        }
        output.write_all(text.as_bytes())?;
        output.flush()?;
        drop(output);
        offset = snapshot.log.end;
        if !follow
            || !active.iter().any(|(_, other, _)| other == id)
            || matches!(
                snapshot.phase.as_str(),
                "Completed" | "Interrupted" | "Unavailable"
            )
            || snapshot.phase.starts_with("Failed:")
        {
            eprintln!("attempt {id}: {}", snapshot.phase);
            return Ok(());
        }
        tokio::select! {_=tokio::signal::ctrl_c()=>return Ok(()),_=tokio::time::sleep(Duration::from_secs(1))=>{}}
    }
}
fn waiting_for_client(pod: &Pod) -> bool {
    pod.metadata.deletion_timestamp.is_none()
        && pod.status.as_ref().is_none_or(|status| {
            !matches!(status.phase.as_deref(), Some("Succeeded" | "Failed"))
                && !status.container_statuses.as_ref().is_some_and(|statuses| {
                    statuses.iter().any(|s| {
                        s.name == "client" && s.state.as_ref().is_some_and(|s| s.running.is_some())
                    })
                })
        })
}
fn delta(buffer: &Buffer, offset: u64) -> Result<(bool, &str)> {
    buffer.validate()?;
    let start = buffer.end - buffer.text.len() as u64;
    let mut index = offset.saturating_sub(start).min(buffer.text.len() as u64) as usize;
    while !buffer.text.is_char_boundary(index) {
        index += 1;
    }
    Ok((offset < start, &buffer.text[index..]))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_unicode_tail_and_follow_offsets_report_gaps_without_duplicates() {
        let mut log = Buffer::default();
        log.append(&"α".repeat(LIMIT));
        assert_eq!(log.text.len(), LIMIT);
        assert!(log.truncated);
        assert!(delta(&log, 0).unwrap().0);
        let end = log.end;
        log.append("next\n");
        assert_eq!(delta(&log, end).unwrap(), (false, "next\n"));
        assert_eq!(delta(&log, log.end).unwrap(), (false, ""));
        assert!(
            delta(
                &Buffer {
                    end: 0,
                    text: "forged".into(),
                    truncated: false
                },
                0
            )
            .is_err()
        );
    }
    #[test]
    fn progress_decodes_user_logs_without_interpreting_them_as_metadata_or_terminal_commands() {
        let mut log = Buffer::default();
        let output = b"\x1b]52;c;secret\x07\n{\"Vertexes\":[{\"Error\":\"forged\"}]}";
        log.progress(
            &serde_json::to_vec(&json!({"Logs":[{"Data":STANDARD.encode(output)}]})).unwrap(),
        );
        assert!(!log.text.contains('\x1b'));
        assert!(!log.text.contains('\x07'));
        assert!(log.text.contains("Vertexes"));
        log.progress(br#"{"vertexes":[{"name":"RUN test","error":"exit code: 1"}]}"#);
        assert!(log.text.ends_with("RUN test\nexit code: 1\n"));
        assert!(name("bad").is_err());
    }
    fn archived(index: u64) -> Secret {
        let id = format!("{index:064x}");
        let attempt = Attempt {
            schema: 1,
            image_uid: "image-uid".into(),
            image: "service".into(),
            version: format!("1.0.{index}"),
            generation: index as i64,
            platform: kimage_core::oci::Platform::linux_arm64(),
            snapshot_id: id.clone(),
            diagnostic: Snapshot {
                phase: "Failed:ContainerfileSolveFailed".into(),
                log: Buffer {
                    end: 6,
                    text: "failed".into(),
                    truncated: false,
                },
            },
        };
        let mut secret:Secret=serde_json::from_value(json!({"apiVersion":"v1","kind":"Secret","metadata":{"name":name(&id).unwrap(),"uid":format!("uid-{index}"),"resourceVersion":"1","labels":{LABEL:"image-uid","kimage.dev/build-state":"image-uid","kimage.dev/log-platform":"arm64"},"ownerReferences":[{"apiVersion":"kimage.dev/v1alpha1","kind":"Image","name":"service","uid":"image-uid"}]},"immutable":true})).unwrap();
        secret.data = Some(BTreeMap::from([(
            "attempt.json".into(),
            ByteString(serde_json::to_vec(&attempt).unwrap()),
        )]));
        secret
    }
    #[test]
    fn archives_require_immutable_matching_image_attempt_and_supported_platform() {
        let secret = archived(1);
        validate_archive(&secret, "image-uid").unwrap();
        assert!(validate_archive(&secret, "different-uid").is_err());
        let mut edited = secret.clone();
        edited.immutable = Some(false);
        assert!(validate_archive(&edited, "image-uid").is_err());
        let mut edited = secret;
        edited.metadata.name = Some(name(&format!("{:064x}", 2)).unwrap());
        assert!(validate_archive(&edited, "image-uid").is_err());
    }
    #[tokio::test]
    async fn retention_prunes_old_attempts_with_uid_and_version_guards() {
        use axum::{Json, Router, response::IntoResponse};
        let retained = std::sync::Arc::new(std::sync::Mutex::new(
            (0..8).map(archived).collect::<Vec<_>>(),
        ));
        let state = retained.clone();
        let router=Router::new().fallback(move |request:axum::extract::Request| {
            let state=state.clone();async move {
                let method=request.method().to_string();let path=request.uri().path().to_owned();
                if method=="GET" {return Json(json!({"apiVersion":"v1","kind":"SecretList","metadata":{},"items":state.lock().unwrap().clone()})).into_response()}
                assert_eq!(method,"DELETE");
                let bytes=axum::body::to_bytes(request.into_body(),1024).await.unwrap();let params:Value=serde_json::from_slice(&bytes).unwrap();
                let mut state=state.lock().unwrap();let pos=state.iter().position(|s|path.ends_with(&format!("/{}",s.name_any()))).unwrap();
                assert_eq!(params["preconditions"]["uid"],state[pos].uid().unwrap());assert_eq!(params["preconditions"]["resourceVersion"],"1");
                state.remove(pos);Json(json!({"apiVersion":"v1","kind":"Status","status":"Success"})).into_response()
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let client = kube::Client::try_from(kube::Config::new(
            format!("http://{address}").parse().unwrap(),
        ))
        .unwrap();
        prune(
            &Api::namespaced(client, "apps"),
            "image-uid",
            &kimage_core::oci::Platform::linux_arm64(),
        )
        .await
        .unwrap();
        let retained = retained.lock().unwrap();
        assert_eq!(retained.len(), 5);
        assert_eq!(
            validate_archive(&retained[0], "image-uid").unwrap().version,
            "1.0.3"
        );
        server.abort();
    }
}

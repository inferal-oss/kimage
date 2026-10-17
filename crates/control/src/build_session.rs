//! Credential-free session process: capture, durable submit acknowledgment, export.
//! Invoked from a fixed Pod definition; normal stdout/stderr never contain inputs.
use crate::{
    containerfile::{Session, SolveSummary},
    input_snapshot::Capture,
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    fs::{self, File},
    io::{Read, Write},
    os::unix::fs::PermissionsExt,
    path::Path,
    time::Duration,
};
use tokio_util::sync::CancellationToken;
const REQUEST: &str = "/configuration/session.json";
fn read<T: serde::de::DeserializeOwned>(path: &str, limit: u64) -> Result<T> {
    let mut bytes = Vec::new();
    File::open(path)?.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= limit, "BuildMetadataTooLarge");
    serde_json::from_slice(&bytes).context("InvalidBuildMetadata")
}
fn write(path: &str, bytes: &[u8], mode: u32) -> Result<()> {
    let pending = format!("{path}.pending");
    let mut file = File::options()
        .create_new(true)
        .write(true)
        .open(&pending)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::set_permissions(&pending, fs::Permissions::from_mode(mode))?;
    fs::rename(pending, path)?;
    Ok(())
}
fn phase(value: &str) -> Result<()> {
    write("/snapshot/phase", value.as_bytes(), 0o444)
}
pub fn prepare() -> Result<()> {
    for path in ["/snapshot", "/sensitive"] {
        fs::set_permissions(path, fs::Permissions::from_mode(0o1777))?;
    }
    let mut source = File::open(std::env::current_exe()?)?;
    let mut target = File::options()
        .create_new(true)
        .write(true)
        .open("/snapshot/session")?;
    kimage_core::oci::copy_hash(&mut source, &mut target, 16 * 1024 * 1024)?;
    target.sync_all()?;
    fs::set_permissions("/snapshot/session", fs::Permissions::from_mode(0o555))?;
    Ok(())
}
pub fn capture_receipt() -> Result<Vec<u8>> {
    let capture: Capture = read("/sensitive/capture.json", 65536)?;
    Ok(serde_json::to_vec(&capture)?)
}
pub fn inspect() -> Result<Value> {
    let current = fs::read_to_string("/snapshot/phase").unwrap_or_else(|_| "Capturing".into());
    ensure!(
        current.len() <= 128
            && current
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b':'),
        "InvalidBuildPhase"
    );
    let summary = if current == "Completed" {
        Some(read::<SolveSummary>("/snapshot/summary.json", 65536)?)
    } else {
        None
    };
    Ok(json!({"phase":current,"summary":summary}))
}
pub fn start(snapshot_id: &str) -> Result<()> {
    let session: Session = read(REQUEST, 768 * 1024)?;
    ensure!(snapshot_id == session.snapshot_id, "StaleBuildSnapshot");
    if let Ok(existing) = fs::read_to_string("/snapshot/submit") {
        ensure!(existing == snapshot_id, "StaleBuildSnapshot");
        return Ok(());
    }
    ensure!(
        fs::read_to_string("/snapshot/phase")? == "Captured",
        "BuildNotCaptured"
    );
    write("/snapshot/submit", snapshot_id.as_bytes(), 0o444)
}
pub fn ready() -> Result<()> {
    ensure!(
        fs::read_to_string("/snapshot/phase")? == "Completed",
        "BuildNotComplete"
    );
    Ok(())
}
pub fn stream() -> Result<()> {
    ready()?;
    let file = File::open("/snapshot/result.oci.tar")?;
    let mut stdout = std::io::stdout().lock();
    kimage_core::oci::copy_hash(
        &mut std::io::BufReader::new(file),
        &mut stdout,
        1024 * 1024 * 1024,
    )?;
    stdout.flush()?;
    Ok(())
}
async fn work(cancel: &CancellationToken) -> Result<()> {
    let session: Session = read(REQUEST, 768 * 1024)?;
    ensure!(
        session.snapshot_id.len() == 64
            && session.snapshot_id.bytes().all(|b| b.is_ascii_hexdigit()),
        "InvalidBuildSnapshotId"
    );
    let build = session.build.clone();
    let mut captured = tokio::task::spawn_blocking(move || {
        crate::input_snapshot::capture(
            &build,
            Path::new("/"),
            Path::new("/snapshot"),
            Path::new("/sensitive"),
        )
    })
    .await??;
    captured.image_inputs = crate::image_inputs::capture(&session, cancel).await?;
    write(
        "/sensitive/capture.json",
        &serde_json::to_vec(&captured)?,
        0o400,
    )?;
    phase("Captured")?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
    loop {
        if let Ok(id) = fs::read_to_string("/snapshot/submit") {
            ensure!(id == session.snapshot_id, "StaleBuildSnapshot");
            break;
        }
        ensure!(
            tokio::time::Instant::now() < deadline,
            "BuildSubmitDeadline"
        );
        tokio::select! {_=cancel.cancelled()=>anyhow::bail!("OperationCancelled"),_=tokio::time::sleep(Duration::from_millis(250))=>{}}
    }
    phase("Submitted")?;
    let summary = crate::containerfile::solve(&session, &captured, cancel).await?;
    write(
        "/snapshot/summary.json",
        &serde_json::to_vec(&summary)?,
        0o444,
    )?;
    phase("Completed")
}
pub async fn run() -> Result<()> {
    let cancel = CancellationToken::new();
    let stop = cancel.clone();
    tokio::spawn(async move {
        crate::serving::shutdown().await;
        stop.cancel();
    });
    if let Err(error) = work(&cancel).await {
        let text = error.to_string();
        let reason = text.split(':').next().unwrap_or("");
        let reason = if !reason.is_empty()
            && reason.len() <= 64
            && reason.bytes().all(|c| c.is_ascii_alphanumeric())
        {
            reason
        } else {
            "BuildInputOrEngineFailed"
        };
        // Persist only the stable reason here. Full BuildKit output is already
        // bounded in its private log and must never be sent to operator stdout.
        let mut log = crate::build_logs::snapshot()
            .map(|s| s.log)
            .unwrap_or_default();
        log.append(&format!("\n{reason}\n"));
        log.persist()?;
        phase(&format!("Failed:{reason}"))?;
    }
    cancel.cancelled().await;
    Ok(())
}

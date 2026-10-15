//! Credential-free PVC capture. stdout in `stream` is exclusively binary content.
use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand};
use std::{fs::File, io::Write, path::PathBuf};
#[derive(Parser)]
#[command(
    name = "kimage-input",
    version,
    about = "Capture bounded kimage build inputs and OCI archives"
)]
struct Args {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    PrepareBuild,
    BuildSession,
    BuildCaptureReceipt,
    BuildPhase,
    BuildStart {
        #[arg(long)]
        snapshot_id: String,
    },
    BuildReady,
    BuildStream,
    Prepare,
    Capture {
        #[arg(long)]
        path: String,
    },
    Stream,
    Ready,
}
#[tokio::main]
async fn main() -> Result<()> {
    match Args::parse().command {
        Command::PrepareBuild => kimage_control::build_session::prepare(),
        Command::BuildSession => kimage_control::build_session::run().await,
        Command::BuildCaptureReceipt => {
            std::io::stdout().write_all(&kimage_control::build_session::capture_receipt()?)?;
            Ok(())
        }
        Command::BuildPhase => {
            println!("{}", kimage_control::build_session::inspect()?);
            Ok(())
        }
        Command::BuildStart { snapshot_id } => kimage_control::build_session::start(&snapshot_id),
        Command::BuildReady => kimage_control::build_session::ready(),
        Command::BuildStream => kimage_control::build_session::stream(),
        Command::Prepare => {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions("/snapshot", std::fs::Permissions::from_mode(0o1777))?;
            Ok(())
        }
        Command::Capture { path } => {
            kimage_control::api::relative(&path)?;
            let mut input = PathBuf::from("/source");
            for part in path.split('/') {
                input.push(part);
                let meta = std::fs::symlink_metadata(&input)?;
                ensure!(!meta.file_type().is_symlink(), "SourceSymlinkDenied");
            }
            ensure!(std::fs::metadata(&input)?.is_file(), "SourceNotRegularFile");
            let mut source = File::open(&input)?;
            let before = source.metadata()?;
            let mut pending = File::create("/snapshot/pending")?;
            kimage_core::oci::copy_hash(&mut source, &mut pending, 1024 * 1024 * 1024)?;
            let after = source.metadata()?;
            ensure!(
                before.len() == after.len() && before.modified()? == after.modified()?,
                "SourceChangedDuringCapture"
            );
            pending.flush()?;
            pending.sync_all()?;
            drop(pending);
            std::fs::rename("/snapshot/pending", "/snapshot/archive")?;
            File::create("/snapshot/ready")?.sync_all()?;
            kimage_control::serving::shutdown().await;
            Ok(())
        }
        Command::Stream => {
            let file = File::open("/snapshot/archive").context("SnapshotNotReady")?;
            let mut stdout = std::io::stdout().lock();
            kimage_core::oci::copy_hash(
                &mut std::io::BufReader::new(file),
                &mut stdout,
                1024 * 1024 * 1024,
            )?;
            stdout.flush()?;
            Ok(())
        }
        Command::Ready => {
            ensure!(
                std::path::Path::new("/snapshot/ready").is_file(),
                "SnapshotNotReady"
            );
            Ok(())
        }
    }
}

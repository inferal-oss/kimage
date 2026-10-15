use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;
#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Verify {
        #[arg(long)]
        archive: PathBuf,
        #[arg(long)]
        root: PathBuf,
    },
    Publish {
        #[arg(long)]
        archive: PathBuf,
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        namespace: String,
        #[arg(long)]
        pod: String,
        #[arg(long)]
        origin: reqwest::Url,
        #[arg(long)]
        repository: String,
        #[arg(long)]
        ca: PathBuf,
        #[arg(long)]
        password_file: PathBuf,
    },
    VerifyRemote {
        #[arg(long)]
        tag: Option<String>,
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        namespace: String,
        #[arg(long)]
        pod: String,
        #[arg(long)]
        origin: reqwest::Url,
        #[arg(long)]
        repository: String,
        #[arg(long)]
        ca: PathBuf,
        #[arg(long)]
        password_file: PathBuf,
    },
    BundleCheck {
        #[arg(long)]
        bundle: PathBuf,
        #[arg(long)]
        trusted_inventory: String,
    },
    Bootstrap(Install),
    Recover(Install),
    Seed(Install),
    Handover(Install),
    Cleanup(Install),
    Uninstall(Cluster),
    ControlPlane {
        #[command(flatten)]
        install: Install,
        #[arg(long)]
        admission_credentials: PathBuf,
    },
    Builder {
        #[command(flatten)]
        install: Install,
        #[arg(long)]
        builder_credentials: PathBuf,
    },
    JoinNode {
        #[command(flatten)]
        install: Install,
        #[arg(long)]
        target_node: String,
        #[arg(long)]
        recover: bool,
    },
    UpgradeServing {
        #[command(flatten)]
        install: Install,
        #[arg(long)]
        previous_bundle: PathBuf,
        #[arg(long)]
        previous_trusted_inventory: String,
    },
    Exporter {
        #[command(flatten)]
        install: Install,
        #[arg(long, required = true)]
        approved_origin: Vec<String>,
    },
    RenewTls {
        #[command(flatten)]
        cluster: Cluster,
        #[arg(long)]
        admission_credentials: PathBuf,
    },
    Schemas,
}
#[derive(Args)]
struct Cluster {
    #[arg(long)]
    application_namespace: Option<String>,
    #[arg(long, value_enum, default_value = "inferal")]
    runtime_profile: kimage_control::install::RuntimeProfile,
    #[arg(long)]
    namespace: String,
    #[arg(long)]
    installation: String,
    #[arg(long)]
    node: String,
    #[arg(long)]
    carrier: String,
    #[arg(long)]
    storage_class: String,
    #[arg(long)]
    pvc: Option<String>,
    #[arg(long)]
    port: u16,
    #[arg(long)]
    credentials: PathBuf,
}
#[derive(Args)]
struct Install {
    #[command(flatten)]
    cluster: Cluster,
    #[arg(long)]
    bundle: PathBuf,
    #[arg(long)]
    trusted_inventory: String,
}
impl Cluster {
    fn options(self) -> kimage_control::install::Options {
        kimage_control::install::Options {
            application_namespace: self.application_namespace,
            runtime_profile: self.runtime_profile,
            namespace: self.namespace,
            installation: self.installation,
            node: self.node,
            carrier: self.carrier,
            storage_class: self.storage_class,
            pvc: self.pvc,
            port: self.port,
            credentials: self.credentials,
        }
    }
}
fn verified(
    archive: &std::path::Path,
    root: &std::path::Path,
) -> Result<kimage_core::oci::VerifiedImage> {
    let expected: kimage_core::oci::Descriptor = serde_json::from_slice(&std::fs::read(root)?)?;
    kimage_core::oci::verify_archive(
        archive,
        &expected,
        &kimage_core::oci::Platform::linux_amd64(),
        kimage_core::oci::Limits::default(),
    )
}
#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().json().init();
    match Cli::parse().command {
        Command::Verify { archive, root } => {
            let image = verified(&archive, &root)?;
            println!("{}", serde_json::to_string(image.root())?);
        }
        Command::Publish {
            archive,
            root,
            namespace,
            pod,
            origin,
            repository,
            ca,
            password_file,
        } => {
            let image = verified(&archive, &root)?;
            let client = kimage_control::kubernetes::client().await?;
            let tunnel =
                kimage_control::tunnel::Tunnel::open(client, &namespace, &pod, 5443).await?;
            let registry = kimage_control::registry::Registry::new(
                origin,
                repository,
                &std::fs::read(ca)?,
                Some(tunnel.address),
                "publisher".into(),
                std::fs::read_to_string(password_file)?.trim_end().into(),
            )?;
            let receipt = registry
                .publish(&image, &tokio_util::sync::CancellationToken::new())
                .await?;
            println!("{}", serde_json::to_string(receipt.root())?);
        }
        Command::BundleCheck {
            bundle,
            trusted_inventory,
        } => {
            kimage_control::bundle::Bundle::open(&bundle, &trusted_inventory)?;
            println!("bundle verified");
        }
        Command::VerifyRemote {
            tag,
            root,
            namespace,
            pod,
            origin,
            repository,
            ca,
            password_file,
        } => {
            let expected: kimage_core::oci::Descriptor =
                serde_json::from_slice(&std::fs::read(root)?)?;
            let tunnel = kimage_control::tunnel::Tunnel::open(
                kimage_control::kubernetes::client().await?,
                &namespace,
                &pod,
                5443,
            )
            .await?;
            let registry = kimage_control::registry::Registry::new(
                origin,
                repository,
                &std::fs::read(ca)?,
                Some(tunnel.address),
                "publisher".into(),
                std::fs::read_to_string(password_file)?.trim_end().into(),
            )?;
            let image = registry
                .fetch_verified(
                    &expected,
                    &kimage_core::oci::Platform::linux_amd64(),
                    &tokio_util::sync::CancellationToken::new(),
                )
                .await?;
            if let Some(tag) = tag {
                anyhow::ensure!(
                    registry.tag_digest(&tag).await?.as_deref() == Some(&expected.digest),
                    "RemoteTagMismatch"
                );
            }
            println!("{}", serde_json::to_string(image.root())?);
        }
        Command::Bootstrap(args) | Command::Recover(args) => {
            let bundle =
                kimage_control::bundle::Bundle::open(&args.bundle, &args.trusted_inventory)?;
            let installer = kimage_control::install::Installer::new(
                kimage_control::kubernetes::client().await?,
                args.cluster.options(),
            )?;
            installer.bootstrap(&bundle).await?;
        }
        Command::Seed(args) => {
            let bundle =
                kimage_control::bundle::Bundle::open(&args.bundle, &args.trusted_inventory)?;
            let installer = kimage_control::install::Installer::new(
                kimage_control::kubernetes::client().await?,
                args.cluster.options(),
            )?;
            installer.seed(&bundle).await?;
        }
        Command::Uninstall(args) => {
            kimage_control::install::Installer::new(
                kimage_control::kubernetes::client().await?,
                args.options(),
            )?
            .uninstall()
            .await?;
        }
        Command::Cleanup(args) => {
            let bundle =
                kimage_control::bundle::Bundle::open(&args.bundle, &args.trusted_inventory)?;
            kimage_control::install::Installer::new(
                kimage_control::kubernetes::client().await?,
                args.cluster.options(),
            )?
            .cleanup_bootstrap(&bundle)
            .await?;
        }
        Command::Handover(args) => {
            let bundle =
                kimage_control::bundle::Bundle::open(&args.bundle, &args.trusted_inventory)?;
            let installer = kimage_control::install::Installer::new(
                kimage_control::kubernetes::client().await?,
                args.cluster.options(),
            )?;
            installer.handover(&bundle).await?;
        }
        Command::ControlPlane {
            install,
            admission_credentials,
        } => {
            let bundle =
                kimage_control::bundle::Bundle::open(&install.bundle, &install.trusted_inventory)?;
            let installer = kimage_control::install::Installer::new(
                kimage_control::kubernetes::client().await?,
                install.cluster.options(),
            )?;
            installer
                .control_plane(&bundle, &admission_credentials)
                .await?;
        }
        Command::Builder {
            install,
            builder_credentials,
        } => {
            let bundle =
                kimage_control::bundle::Bundle::open(&install.bundle, &install.trusted_inventory)?;
            kimage_control::install::Installer::new(
                kimage_control::kubernetes::client().await?,
                install.cluster.options(),
            )?
            .builder(&bundle, &builder_credentials)
            .await?;
        }
        Command::JoinNode {
            install,
            target_node,
            recover,
        } => {
            let bundle =
                kimage_control::bundle::Bundle::open(&install.bundle, &install.trusted_inventory)?;
            kimage_control::install::Installer::new(
                kimage_control::kubernetes::client().await?,
                install.cluster.options(),
            )?
            .join_node(&bundle, &target_node, recover)
            .await?;
        }
        Command::UpgradeServing {
            install,
            previous_bundle,
            previous_trusted_inventory,
        } => {
            let next =
                kimage_control::bundle::Bundle::open(&install.bundle, &install.trusted_inventory)?;
            let previous = kimage_control::bundle::Bundle::open(
                &previous_bundle,
                &previous_trusted_inventory,
            )?;
            kimage_control::install::Installer::new(
                kimage_control::kubernetes::client().await?,
                install.cluster.options(),
            )?
            .upgrade_serving(&previous, &next)
            .await?;
        }
        Command::Exporter {
            install,
            approved_origin,
        } => {
            let bundle =
                kimage_control::bundle::Bundle::open(&install.bundle, &install.trusted_inventory)?;
            kimage_control::install::Installer::new(
                kimage_control::kubernetes::client().await?,
                install.cluster.options(),
            )?
            .exporter(&bundle, &approved_origin)
            .await?;
        }
        Command::RenewTls {
            cluster,
            admission_credentials,
        } => {
            let installer = kimage_control::install::Installer::new(
                kimage_control::kubernetes::client().await?,
                cluster.options(),
            )?;
            println!("{}", installer.renew_tls(&admission_credentials).await?);
        }
        Command::Schemas => println!(
            "{}",
            serde_json::to_string_pretty(&kimage_control::schema::definitions()?)?
        ),
    }
    Ok(())
}

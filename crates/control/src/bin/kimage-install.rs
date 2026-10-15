use anyhow::Result;
use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand};
use std::path::PathBuf;
#[derive(Parser)]
#[command(
    name = "kimage",
    version,
    about = "Install, verify, upgrade, and recover a kimage installation through Kubernetes APIs"
)]
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
    /// Download and verify an official release without accessing Kubernetes.
    BundleFetch(BundleSource),
    /// Verify a local bundle or the selected cached/official release.
    BundleCheck(BundleSource),
    /// Fetch the bundle and run bootstrap, seed, handover, control plane, and builder.
    Install {
        #[command(flatten)]
        install: PhaseOptions,
        #[arg(long)]
        admission_credentials: PathBuf,
        #[arg(long)]
        builder_credentials: PathBuf,
    },
    Bootstrap(PhaseOptions),
    Recover(PhaseOptions),
    Seed(PhaseOptions),
    Handover(PhaseOptions),
    Cleanup(PhaseOptions),
    Uninstall(Cluster),
    ControlPlane {
        #[command(flatten)]
        install: PhaseOptions,
        #[arg(long)]
        admission_credentials: PathBuf,
    },
    Builder {
        #[command(flatten)]
        install: PhaseOptions,
        #[arg(long)]
        builder_credentials: PathBuf,
    },
    JoinNode {
        #[command(flatten)]
        install: PhaseOptions,
        #[arg(long)]
        target_node: String,
        #[arg(long)]
        recover: bool,
    },
    UpgradeServing {
        #[command(flatten)]
        install: PhaseOptions,
        #[arg(
            long,
            required_unless_present = "previous_release",
            conflicts_with = "previous_release",
            requires = "previous_trusted_inventory"
        )]
        previous_bundle: Option<PathBuf>,
        #[arg(long)]
        previous_trusted_inventory: Option<String>,
        #[arg(long, required_unless_present = "previous_bundle")]
        previous_release: Option<String>,
    },
    Exporter {
        #[command(flatten)]
        install: PhaseOptions,
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
struct PhaseOptions {
    #[command(flatten)]
    cluster: Cluster,
    #[command(flatten)]
    source: BundleSource,
}

#[derive(Args)]
struct BundleSource {
    /// Use an independently trusted local bundle instead of fetching a release.
    #[arg(long, conflicts_with = "release", requires = "trusted_inventory")]
    bundle: Option<PathBuf>,
    /// Pin the inventory as sha256:<64 lowercase hex digits>; required for local bundles.
    #[arg(long)]
    trusted_inventory: Option<String>,
    /// Exact official release; defaults to this CLI package version, never latest.
    #[arg(long, value_name = "VERSION")]
    release: Option<String>,
    /// Override the private, verified release bundle cache directory.
    #[arg(long)]
    bundle_cache: Option<PathBuf>,
    /// Disable bundle downloads; require cached/local bytes. Cluster commands still use Kubernetes.
    #[arg(long)]
    offline: bool,
}
impl BundleSource {
    async fn resolve(&self) -> Result<kimage_control::bundle::Bundle> {
        kimage_control::release_bundle::resolve(
            self.bundle.as_deref(),
            self.trusted_inventory.as_deref(),
            self.release.as_deref(),
            self.bundle_cache.as_deref(),
            self.offline,
        )
        .await
    }
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
    tracing_subscriber::fmt()
        .json()
        .with_writer(std::io::stderr)
        .init();
    let program = std::env::args_os().next().unwrap_or_default();
    let name = if std::path::Path::new(&program)
        .file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with("kimage-install"))
    {
        "kimage-install"
    } else {
        "kimage"
    };
    let matches = Cli::command().name(name).get_matches();
    match Cli::from_arg_matches(&matches)?.command {
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
        Command::BundleFetch(source) => {
            let bundle = source.resolve().await?;
            println!(
                "{}",
                serde_json::json!({"bundle": bundle.root, "trustedInventory": bundle.digest})
            );
        }
        Command::BundleCheck(source) => {
            source.resolve().await?;
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
        Command::Install {
            install,
            admission_credentials,
            builder_credentials,
        } => {
            let bundle = install.source.resolve().await?;
            kimage_control::install::Installer::new(
                kimage_control::kubernetes::client().await?,
                install.cluster.options(),
            )?
            .install(&bundle, &admission_credentials, &builder_credentials)
            .await?;
        }
        Command::Bootstrap(args) | Command::Recover(args) => {
            let bundle = args.source.resolve().await?;
            let installer = kimage_control::install::Installer::new(
                kimage_control::kubernetes::client().await?,
                args.cluster.options(),
            )?;
            installer.bootstrap(&bundle).await?;
        }
        Command::Seed(args) => {
            let bundle = args.source.resolve().await?;
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
            let bundle = args.source.resolve().await?;
            kimage_control::install::Installer::new(
                kimage_control::kubernetes::client().await?,
                args.cluster.options(),
            )?
            .cleanup_bootstrap(&bundle)
            .await?;
        }
        Command::Handover(args) => {
            let bundle = args.source.resolve().await?;
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
            let bundle = install.source.resolve().await?;
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
            let bundle = install.source.resolve().await?;
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
            let bundle = install.source.resolve().await?;
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
            previous_release,
        } => {
            let next = install.source.resolve().await?;
            let previous = kimage_control::release_bundle::resolve(
                previous_bundle.as_deref(),
                previous_trusted_inventory.as_deref(),
                previous_release.as_deref(),
                install.source.bundle_cache.as_deref(),
                install.source.offline,
            )
            .await?;
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
            let bundle = install.source.resolve().await?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_install_defaults_to_automatic_bundle_selection() {
        let args = [
            "kimage",
            "install",
            "--namespace",
            "system",
            "--application-namespace",
            "apps",
            "--installation",
            "test",
            "--node",
            "node",
            "--carrier",
            "image@sha256:digest",
            "--storage-class",
            "local",
            "--port",
            "5001",
            "--credentials",
            "registry",
            "--admission-credentials",
            "admission",
            "--builder-credentials",
            "builder",
        ];
        let cli = Cli::try_parse_from(args).unwrap();
        let Command::Install { install, .. } = cli.command else {
            panic!("expected install")
        };
        assert!(install.source.bundle.is_none());
        assert!(install.source.release.is_none());
        assert!(!install.source.offline);
        assert!(Cli::try_parse_from(&args[..args.len() - 2]).is_err());
    }

    #[test]
    fn bundle_cli_preserves_local_pins_and_allows_exact_cached_releases() {
        for args in [
            vec!["ki", "bundle-fetch"],
            vec![
                "ki",
                "bundle-fetch",
                "--release",
                "0.2.0",
                "--offline",
                "--bundle-cache",
                "cache",
            ],
            vec![
                "ki",
                "bundle-check",
                "--bundle",
                "local",
                "--trusted-inventory",
                "sha256:pin",
            ],
        ] {
            assert!(Cli::try_parse_from(args).is_ok());
        }
        for args in [
            vec!["ki", "bundle-check", "--bundle", "local"],
            vec![
                "ki",
                "bundle-check",
                "--bundle",
                "local",
                "--trusted-inventory",
                "pin",
                "--release",
                "0.2.0",
            ],
        ] {
            assert!(Cli::try_parse_from(args).is_err());
        }
    }
}

use anyhow::Result;
use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand};
use std::path::PathBuf;
#[derive(Parser)]
#[command(
    name = "kimage",
    version,
    about = "Install kimage using versioned component images and ordinary Kubernetes resources"
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
    #[command(hide = true)]
    BundleFetch(BundleSource),
    /// Verify a local bundle or the selected cached/official release.
    #[command(hide = true)]
    BundleCheck(BundleSource),
    /// Install ordinary components pulled directly from the release image registry.
    Install(ImageInstall),
    /// Remove an installation while retaining images and registry storage.
    Uninstall(RemoveInstallation),
    /// Retained bootstrap workflow for independently trusted legacy bundles.
    #[command(hide = true)]
    LegacyInstall {
        #[command(flatten)]
        install: PhaseOptions,
        #[arg(long)]
        admission_credentials: PathBuf,
        #[arg(long)]
        builder_credentials: PathBuf,
    },
    #[command(hide = true)]
    Bootstrap(PhaseOptions),
    #[command(hide = true)]
    Recover(PhaseOptions),
    #[command(hide = true)]
    Seed(PhaseOptions),
    #[command(hide = true)]
    Handover(PhaseOptions),
    #[command(hide = true)]
    Cleanup(PhaseOptions),
    #[command(hide = true)]
    LegacyUninstall(Cluster),
    #[command(hide = true)]
    ControlPlane {
        #[command(flatten)]
        install: PhaseOptions,
        #[arg(long)]
        admission_credentials: PathBuf,
    },
    #[command(hide = true)]
    Builder {
        #[command(flatten)]
        install: PhaseOptions,
        #[arg(long)]
        builder_credentials: PathBuf,
    },
    #[command(hide = true)]
    JoinNode {
        #[command(flatten)]
        install: PhaseOptions,
        #[arg(long)]
        target_node: String,
        #[arg(long)]
        recover: bool,
    },
    #[command(hide = true)]
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
    #[command(hide = true)]
    Exporter {
        #[command(flatten)]
        install: PhaseOptions,
        #[arg(long, required = true)]
        approved_origin: Vec<String>,
    },
    #[command(hide = true)]
    RenewTls {
        #[command(flatten)]
        cluster: Cluster,
        #[arg(long)]
        admission_credentials: PathBuf,
    },
    Schemas,
}
#[derive(Args)]
struct ImageInstall {
    #[command(flatten)]
    cluster: ImageCluster,
    /// Exact release whose small component catalog selects immutable image digests.
    #[arg(long)]
    release: Option<String>,
    /// Independently selected catalog, for a verified local copy or registry mirror.
    #[arg(long)]
    image_catalog: Option<PathBuf>,
}
#[derive(Args)]
struct RemoveInstallation {
    #[arg(long, default_value = "kimage-system")]
    namespace: String,
    #[arg(long, default_value = "kimage")]
    installation: String,
}
#[derive(Args)]
struct ImageCluster {
    #[arg(long, default_value = "kimage-system")]
    namespace: String,
    #[arg(long, default_value = "kimage")]
    installation: String,
    #[arg(long, value_enum, default_value = "namespace")]
    scope: kimage_control::install::InstallScope,
    /// Defaults to the current kubeconfig namespace for namespace scope.
    #[arg(long)]
    application_namespace: Option<String>,
    #[arg(long = "exclude-namespace")]
    excluded_namespaces: Vec<String>,
    #[arg(long, value_enum, default_value = "inferal")]
    runtime_profile: kimage_control::install::RuntimeProfile,
    /// Optional registry placement; workers and node proxies are discovered automatically.
    #[arg(long)]
    registry_node: Option<String>,
    /// Omit to use the cluster's default StorageClass.
    #[arg(long)]
    storage_class: Option<String>,
    #[arg(long, default_value = "1Gi")]
    storage_size: String,
    #[arg(long)]
    pvc: Option<String>,
    #[arg(long, default_value_t = 17701)]
    port: u16,
}
impl ImageCluster {
    fn options(&self, default_namespace: &str) -> Result<kimage_control::pull_install::Options> {
        let options = kimage_control::pull_install::Options {
            namespace: self.namespace.clone(),
            installation: self.installation.clone(),
            scope: self.scope,
            application_namespace: match self.scope {
                kimage_control::install::InstallScope::Namespace => Some(
                    self.application_namespace
                        .clone()
                        .unwrap_or_else(|| default_namespace.into()),
                ),
                kimage_control::install::InstallScope::Cluster => {
                    self.application_namespace.clone()
                }
            },
            excluded_namespaces: self.excluded_namespaces.iter().cloned().collect(),
            runtime_profile: self.runtime_profile,
            registry_node: self.registry_node.clone(),
            storage_class: self.storage_class.clone(),
            storage_size: self.storage_size.clone(),
            existing_claim: self.pvc.clone(),
            port: self.port,
        };
        options.validate()?;
        Ok(options)
    }
}
#[derive(Args)]
struct Cluster {
    /// Manage one application namespace (default) or all ordinary namespaces.
    #[arg(long, value_enum, default_value = "namespace")]
    scope: kimage_control::install::InstallScope,
    /// Leave these namespaces outside a cluster-wide installation.
    #[arg(long = "exclude-namespace", value_name = "NAMESPACE")]
    excluded_namespaces: Vec<String>,
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
    /// Bundle platform; cluster phases select the target node automatically.
    #[arg(long, value_name = "linux/amd64|linux/arm64")]
    platform: Option<kimage_core::oci::Platform>,
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
        if let Some(local) = &self.bundle
            && self.platform.is_none()
        {
            return kimage_control::bundle::Bundle::open(
                local,
                self.trusted_inventory
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("LocalBundleRequiresTrustedInventory"))?,
            );
        }
        self.resolve_platform(
            self.platform
                .as_ref()
                .unwrap_or(&kimage_core::oci::Platform::native()),
        )
        .await
    }
    async fn resolve_for_node(
        &self,
        cluster: &Cluster,
        target: Option<&str>,
    ) -> Result<kimage_control::bundle::Bundle> {
        let platform = kimage_control::install::Installer::node_platform(
            kimage_control::kubernetes::client().await?,
            target.unwrap_or(&cluster.node),
        )
        .await?;
        anyhow::ensure!(
            self.platform.as_ref().is_none_or(|p| p == &platform),
            "RequestedPlatformDoesNotMatchNode"
        );
        self.resolve_platform(&platform).await
    }
    async fn resolve_platform(
        &self,
        platform: &kimage_core::oci::Platform,
    ) -> Result<kimage_control::bundle::Bundle> {
        kimage_control::release_bundle::resolve_for_platform(
            self.bundle.as_deref(),
            self.trusted_inventory.as_deref(),
            self.release.as_deref(),
            self.bundle_cache.as_deref(),
            self.offline,
            platform,
        )
        .await
    }
}
impl Cluster {
    fn options(self) -> kimage_control::install::Options {
        kimage_control::install::Options {
            scope: self.scope,
            excluded_namespaces: self.excluded_namespaces.into_iter().collect(),
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
        &kimage_core::oci::Platform::automatic(),
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
                    &kimage_core::oci::Platform::automatic(),
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
        Command::Install(args) => {
            let release = args.release.as_deref().unwrap_or(env!("CARGO_PKG_VERSION"));
            let catalog = kimage_control::component_images::ImageCatalog::resolve(
                release,
                args.image_catalog.as_deref(),
            )
            .await?;
            let namespace = kube::Config::infer().await?.default_namespace;
            let options = args.cluster.options(&namespace)?;
            kimage_control::pull_install::apply::install(
                kimage_control::kubernetes::client().await?,
                &options,
                &catalog,
            )
            .await?;
            println!(
                "kimage {} ready in {}",
                options.installation, options.namespace
            );
        }
        Command::Uninstall(args) => {
            kimage_control::pull_install::apply::uninstall_installed(
                kimage_control::kubernetes::client().await?,
                &args.namespace,
                &args.installation,
            )
            .await?;
            println!(
                "kimage {} removed; image resources and registry storage retained",
                args.installation
            );
        }
        Command::LegacyInstall {
            install,
            admission_credentials,
            builder_credentials,
        } => {
            let bundle = install
                .source
                .resolve_for_node(&install.cluster, None)
                .await?;
            let installer = kimage_control::install::Installer::new(
                kimage_control::kubernetes::client().await?,
                install.cluster.options(),
            )?;
            let mut additional = Vec::new();
            for (platform, node) in installer.native_worker_nodes().await? {
                if node == installer.options.node {
                    continue;
                }
                anyhow::ensure!(
                    install.source.bundle.is_none(),
                    "AdditionalNativeBundleRequired: use the matching per-platform bundle with join-node and builder, or omit --bundle for automatic fetching"
                );
                let native = install.source.resolve_platform(&platform).await?;
                let mut options = installer.options.clone();
                options.node = node.clone();
                kimage_control::install::Installer::new(installer.client.clone(), options)?
                    .preflight_bundle(&native)
                    .await?;
                additional.push((node, native));
            }
            installer
                .install(&bundle, &admission_credentials, &builder_credentials)
                .await?;
            for (node, native) in additional {
                installer
                    .install_native_worker(&native, &node, &builder_credentials)
                    .await?;
            }
        }
        Command::Bootstrap(args) | Command::Recover(args) => {
            let bundle = args.source.resolve_for_node(&args.cluster, None).await?;
            let installer = kimage_control::install::Installer::new(
                kimage_control::kubernetes::client().await?,
                args.cluster.options(),
            )?;
            installer.bootstrap(&bundle).await?;
        }
        Command::Seed(args) => {
            let bundle = args.source.resolve_for_node(&args.cluster, None).await?;
            let installer = kimage_control::install::Installer::new(
                kimage_control::kubernetes::client().await?,
                args.cluster.options(),
            )?;
            installer.seed(&bundle).await?;
        }
        Command::LegacyUninstall(args) => {
            kimage_control::install::Installer::new(
                kimage_control::kubernetes::client().await?,
                args.options(),
            )?
            .uninstall()
            .await?;
        }
        Command::Cleanup(args) => {
            let bundle = args.source.resolve_for_node(&args.cluster, None).await?;
            kimage_control::install::Installer::new(
                kimage_control::kubernetes::client().await?,
                args.cluster.options(),
            )?
            .cleanup_bootstrap(&bundle)
            .await?;
        }
        Command::Handover(args) => {
            let bundle = args.source.resolve_for_node(&args.cluster, None).await?;
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
            let bundle = install
                .source
                .resolve_for_node(&install.cluster, None)
                .await?;
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
            let bundle = install
                .source
                .resolve_for_node(&install.cluster, None)
                .await?;
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
            let bundle = install
                .source
                .resolve_for_node(&install.cluster, Some(&target_node))
                .await?;
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
            let next = install
                .source
                .resolve_for_node(&install.cluster, None)
                .await?;
            let previous = kimage_control::release_bundle::resolve_for_platform(
                previous_bundle.as_deref(),
                previous_trusted_inventory.as_deref(),
                previous_release.as_deref(),
                install.source.bundle_cache.as_deref(),
                install.source.offline,
                &next.inventory.platform,
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
            let bundle = install
                .source
                .resolve_for_node(&install.cluster, None)
                .await?;
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
    fn legacy_install_retains_explicit_bootstrap_inputs() {
        let args = [
            "kimage",
            "legacy-install",
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
        let Command::LegacyInstall { install, .. } = cli.command else {
            panic!("expected install")
        };
        assert!(install.source.bundle.is_none());
        assert!(install.source.release.is_none());
        assert!(!install.source.offline);
        assert!(Cli::try_parse_from(&args[..args.len() - 2]).is_err());
    }

    #[test]
    fn default_install_needs_no_bundle_carrier_node_or_credential_script() {
        let cli = Cli::try_parse_from(["kimage", "install"]).unwrap();
        let Command::Install(args) = cli.command else {
            panic!("expected ordinary image install")
        };
        assert!(args.release.is_none() && args.image_catalog.is_none());
        let options = args.cluster.options("apps").unwrap();
        assert_eq!(options.namespace, "kimage-system");
        assert_eq!(options.installation, "kimage");
        assert_eq!(options.application_namespace.as_deref(), Some("apps"));
        assert!(options.registry_node.is_none() && options.storage_class.is_none());
        assert!(
            Cli::try_parse_from(["kimage", "install", "--carrier", "cached@sha256:old"]).is_err()
        );
        assert!(Cli::try_parse_from(["kimage", "uninstall"]).is_ok());
        let cli = Cli::try_parse_from([
            "kimage",
            "install",
            "--scope",
            "cluster",
            "--exclude-namespace",
            "skip",
        ])
        .unwrap();
        let Command::Install(args) = cli.command else {
            unreachable!()
        };
        let options = args.cluster.options("apps").unwrap();
        assert!(
            options.application_namespace.is_none() && options.excluded_namespaces.contains("skip")
        );
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

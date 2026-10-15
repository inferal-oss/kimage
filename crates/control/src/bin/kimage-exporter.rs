use anyhow::{Result, ensure};
use clap::Parser;
use futures::StreamExt;
use std::{collections::BTreeSet, path::PathBuf, sync::Arc, time::Duration};
#[derive(Parser)]
#[command(
    name = "kimage-exporter",
    version,
    about = "Export retained kimage releases to approved external registries"
)]
struct Args {
    #[arg(
        long,
        required_unless_present = "cluster_wide",
        conflicts_with = "cluster_wide"
    )]
    namespace: Option<String>,
    #[arg(long)]
    cluster_wide: bool,
    #[arg(long = "exclude-namespace", requires = "cluster_wide")]
    excluded_namespaces: Vec<String>,
    #[arg(long)]
    system_namespace: String,
    #[arg(long)]
    installation: String,
    #[arg(long)]
    internal_origin: reqwest::Url,
    #[arg(long)]
    internal_ca: PathBuf,
    #[arg(long)]
    internal_password: PathBuf,
    #[arg(long, required = true)]
    approved_origin: Vec<String>,
}
#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().json().init();
    let a = Args::parse();
    ensure!(a.approved_origin.len() <= 8, "TooManyApprovedExportOrigins");
    let mut approved = BTreeSet::new();
    for value in a.approved_origin {
        let url: reqwest::Url = value.parse()?;
        ensure!(
            url.scheme() == "https"
                && url.username().is_empty()
                && url.password().is_none()
                && url.path() == "/"
                && url.query().is_none()
                && url.fragment().is_none(),
            "InvalidApprovedExportOrigin"
        );
        approved.insert(url.to_string());
    }
    let client = kimage_control::kubernetes::client().await?;
    let cancel = tokio_util::sync::CancellationToken::new();
    let scope = kimage_core::scope::NamespaceScope {
        system_namespace: a.system_namespace.clone(),
        application_namespace: a.namespace.clone(),
        cluster_wide: a.cluster_wide,
        excluded_namespaces: a.excluded_namespaces.into_iter().collect(),
    };
    scope.validate()?;
    let config = Arc::new(kimage_control::exporter::Config {
        scope: Some(scope),
        client: client.clone(),
        namespace: a.namespace.clone().unwrap_or_else(|| "*".into()),
        system_namespace: a.system_namespace,
        installation: a.installation,
        internal_origin: a.internal_origin,
        internal_ca: a.internal_ca,
        internal_password: a.internal_password,
        approved_origins: approved,
        cancel: cancel.clone(),
        concurrency: Arc::new(tokio::sync::Semaphore::new(2)),
    });
    let listener = kimage_control::serving::listener("[::]:8080".parse()?)?;
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            axum::Router::new().route(
                "/readyz",
                axum::routing::get(|| async { axum::http::StatusCode::OK }),
            ),
        )
        .await;
    });
    let api: kube::Api<kimage_control::api::Image> = if a.cluster_wide {
        kube::Api::all(client)
    } else {
        kube::Api::namespaced(client, a.namespace.as_ref().unwrap())
    };
    let controller = kube::runtime::Controller::new(api, kube::runtime::watcher::Config::default())
        .run(
            |image, config| async move {
                kimage_control::exporter::reconcile(image, config)
                    .await
                    .map_err(|_| std::io::Error::other("ExportReconcileRetry"))
            },
            |_, _, _| kube::runtime::controller::Action::requeue(Duration::from_secs(5)),
            config,
        )
        .for_each(|value| async move {
            if value.is_err() {
                tracing::warn!(reason = "ExportReconcileRetry");
            }
        });
    tokio::select! {_=kimage_control::serving::shutdown()=>cancel.cancel(),_=controller=>{}}
    Ok(())
}

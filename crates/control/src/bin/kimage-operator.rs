use anyhow::Result;
use clap::Parser;
use futures::StreamExt;
use kube::{
    Api, ResourceExt,
    runtime::{Controller, controller::Action, watcher},
};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
#[derive(Parser)]
#[command(
    name = "kimage-operator",
    version,
    about = "Reconcile kimage images, builds, and retained releases"
)]
struct Args {
    #[arg(long, requires = "builder_image")]
    installation: Option<String>,
    #[arg(long, requires = "installation")]
    builder_image: Option<String>,
    #[arg(
        long,
        required_unless_present = "cluster_wide",
        conflicts_with = "cluster_wide"
    )]
    namespace: Option<String>,
    #[arg(long, requires = "system_namespace")]
    cluster_wide: bool,
    #[arg(long = "exclude-namespace", requires = "cluster_wide")]
    excluded_namespaces: Vec<String>,
    #[arg(long)]
    node: String,
    #[arg(long)]
    input_image: String,
    #[arg(long)]
    registry_origin: reqwest::Url,
    #[arg(long)]
    registry_ca: PathBuf,
    #[arg(long)]
    publisher_password: PathBuf,
    #[arg(long)]
    registry_pod: Option<String>,
    #[arg(long)]
    schema_once: bool,
    #[arg(long)]
    system_namespace: Option<String>,
}
#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().json().init();
    if std::env::args().nth(1).as_deref() == Some("provision") {
        let args: Vec<_> = std::env::args().skip(2).collect();
        anyhow::ensure!(
            args.len() == 2 && args[0] == "--configuration",
            "Usage: kimage-operator provision --configuration PATH"
        );
        return kimage_control::gitops::run(std::path::Path::new(&args[1])).await;
    }
    let a = Args::parse();
    let namespace = a.namespace.clone().unwrap_or_else(|| "*".into());
    let scope = a
        .system_namespace
        .as_ref()
        .map(|system| kimage_core::scope::NamespaceScope {
            system_namespace: system.clone(),
            application_namespace: a.namespace.clone(),
            cluster_wide: a.cluster_wide,
            excluded_namespaces: a.excluded_namespaces.iter().cloned().collect(),
        });
    if let Some(scope) = &scope {
        scope.validate()?;
    }
    let client = kimage_control::kubernetes::client().await?;
    let ready = Arc::new(AtomicBool::new(false));
    let health = ready.clone();
    let listener = kimage_control::serving::listener("[::]:8080".parse()?)?;
    tokio::spawn(async move {
        let router = axum::Router::new()
            .route(
                "/metrics",
                axum::routing::get(kimage_control::metrics::render),
            )
            .route(
                "/livez",
                axum::routing::get(|| async { axum::http::StatusCode::OK }),
            )
            .route(
                "/readyz",
                axum::routing::get(move || {
                    let health = health.clone();
                    async move {
                        if health.load(Ordering::Relaxed) {
                            axum::http::StatusCode::OK
                        } else {
                            axum::http::StatusCode::SERVICE_UNAVAILABLE
                        }
                    }
                }),
            );
        let _ = axum::serve(listener, router).await;
    });
    let identity = if let Ok(uid) = std::env::var("POD_UID") {
        uid
    } else {
        let mut nonce = [0_u8; 16];
        getrandom::fill(&mut nonce).map_err(|_| anyhow::anyhow!("OperatorIdentityUnavailable"))?;
        format!("native-{}", hex::encode(nonce))
    };
    let schema = kimage_control::schema::SchemaManager::new(
        client.clone(),
        a.system_namespace
            .clone()
            .unwrap_or_else(|| namespace.clone()),
        identity,
    );
    let cancel = tokio_util::sync::CancellationToken::new();
    let stop = cancel.clone();
    tokio::spawn(async move {
        kimage_control::serving::shutdown().await;
        stop.cancel();
    });
    loop {
        match schema.reconcile().await {
            Ok(()) => break,
            Err(error) => {
                kimage_control::metrics::schema_pending();
                let message = error.to_string();
                let reason = message.split(':').next().unwrap_or("SchemaPhasePending");
                let reason =
                    if reason.len() <= 128 && reason.bytes().all(|b| b.is_ascii_alphanumeric()) {
                        reason
                    } else {
                        "SchemaApiOrPermissionFailure"
                    };
                tracing::warn!(reason, "custom reconciliation remains disabled");
                if schema.diagnostic(false, reason).await.is_err() {
                    tracing::warn!(reason = "SchemaDiagnosticUnavailable");
                }
                if a.schema_once {
                    anyhow::bail!("{reason}");
                }
                tokio::select! { _ = cancel.cancelled() => return Ok(()), _ = tokio::time::sleep(Duration::from_secs(5)) => {} }
            }
        }
    }
    if a.schema_once {
        return Ok(());
    }
    ready.store(true, Ordering::Relaxed);
    let sampler_client = client.clone();
    let sampler_namespace = namespace.clone();
    let sampler_cancel = cancel.clone();
    let sampler_scope = scope.clone();
    tokio::spawn(async move {
        loop {
            if !matches!(
                tokio::time::timeout(
                    Duration::from_secs(30),
                    kimage_control::metrics::sample_catalog_scoped(
                        &sampler_client,
                        &sampler_namespace,
                        sampler_scope.as_ref()
                    )
                )
                .await,
                Ok(Ok(()))
            ) {
                tracing::warn!(reason = "MetricsCatalogUnavailable");
            }
            tokio::select! { _=sampler_cancel.cancelled()=>break, _=tokio::time::sleep(Duration::from_secs(10))=>{} }
        }
    });
    let config = Arc::new(kimage_control::reconcile::Config {
        scope: scope.clone(),
        direct: match (a.installation.clone(), a.builder_image) {
            (Some(installation), Some(builder_image)) => Some(kimage_control::direct::Settings {
                architecture: None,
                system_namespace: a
                    .system_namespace
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("SystemNamespaceRequiredForBuilder"))?,
                installation,
                builder_image,
            }),
            (None, None) => None,
            _ => anyhow::bail!("IncompleteBuilderConfiguration"),
        },
        client: client.clone(),
        namespace: namespace.clone(),
        node: a.node,
        input_image: a.input_image,
        registry_origin: a.registry_origin,
        registry_ca: a.registry_ca,
        publisher_password: a.publisher_password,
        registry_pod: a.registry_pod,
        cancel: cancel.clone(),
        concurrency: Arc::new(tokio::sync::Semaphore::new(2)),
    });
    let node_config = config.clone();
    tokio::spawn(async move {
        loop {
            if let Err(error) = kimage_control::node_reconcile::once(node_config.clone()).await {
                let _ = error;
                tracing::warn!(reason = "NodeReconciliationPending");
            }
            tokio::select! {_=node_config.cancel.cancelled()=>break,_=tokio::time::sleep(Duration::from_secs(10))=>{}}
        }
    });
    let api: Api<kimage_control::api::Image> = if a.cluster_wide {
        Api::all(client.clone())
    } else {
        Api::namespaced(client.clone(), &namespace)
    };
    let controller = Controller::new(api, watcher::Config::default());
    let maps_store = controller.store();
    let secrets_store = maps_store.clone();
    let controller = controller
        .watches(
            if a.cluster_wide {
                Api::<k8s_openapi::api::core::v1::ConfigMap>::all(client.clone())
            } else {
                Api::<k8s_openapi::api::core::v1::ConfigMap>::namespaced(client.clone(), &namespace)
            },
            watcher::Config::default(),
            move |object| {
                dependent_images(
                    &maps_store,
                    &object.name_any(),
                    object.namespace().as_deref(),
                    false,
                )
            },
        )
        .watches(
            if a.cluster_wide {
                Api::<k8s_openapi::api::core::v1::Secret>::all(client.clone())
            } else {
                Api::<k8s_openapi::api::core::v1::Secret>::namespaced(client.clone(), &namespace)
            },
            watcher::Config::default(),
            move |object| {
                dependent_images(
                    &secrets_store,
                    &object.name_any(),
                    object.namespace().as_deref(),
                    true,
                )
            },
        );
    let controller = if let Some(system) = a.system_namespace.as_deref() {
        let demand_store = controller.store();
        let policy_store = controller.store();
        let selector = a
            .installation
            .as_deref()
            .map(|i| format!("kimage.dev/installation={i}"))
            .unwrap_or_default();
        controller
            .watches(
                Api::<kimage_control::configuration::ImageDemand>::namespaced(
                    client.clone(),
                    system,
                ),
                watcher::Config::default().labels(&selector),
                move |demand| {
                    demand_store
                        .state()
                        .into_iter()
                        .filter(|image| {
                            image.name_any() == demand.spec.image_name
                                && image.namespace().as_deref() == Some(&demand.spec.namespace)
                                && image.uid().as_deref() == Some(&demand.spec.image_uid)
                        })
                        .map(|image| kube::runtime::reflector::ObjectRef::from_obj(image.as_ref()))
                        .collect::<Vec<_>>()
                },
            )
            .watches(
                Api::<k8s_openapi::api::core::v1::ConfigMap>::namespaced(client.clone(), system),
                watcher::Config::default().labels(&selector),
                move |_| {
                    policy_store
                        .state()
                        .into_iter()
                        .map(|image| kube::runtime::reflector::ObjectRef::from_obj(image.as_ref()))
                        .collect::<Vec<_>>()
                },
            )
    } else {
        controller
    };
    let controller = controller
        .run(
            |image, config| async move {
                let work = async {
                    if let Some(settings) = &config.direct
                        && kimage_control::lifecycle::active(
                            config.client.clone(),
                            &settings.system_namespace,
                            &settings.installation,
                        )
                        .await?
                    {
                        return Ok(Action::requeue(Duration::from_secs(3)));
                    }
                    kimage_control::reconcile::reconcile(image, config).await
                };
                work.await
                    .map_err(|error: anyhow::Error| std::io::Error::other(error.to_string()))
            },
            |_, _, _| Action::requeue(Duration::from_secs(10)),
            config,
        )
        .for_each(|result| async move {
            if result.is_err() {
                tracing::warn!(reason = "ReconcileRetry");
            }
        });
    tokio::select! {_=cancel.cancelled()=>{},_=controller=>{}};
    Ok(())
}

fn dependent_images(
    store: &kube::runtime::reflector::Store<kimage_control::api::Image>,
    name: &str,
    namespace: Option<&str>,
    secret: bool,
) -> Vec<kube::runtime::reflector::ObjectRef<kimage_control::api::Image>> {
    store
        .state()
        .into_iter()
        .filter_map(|image| {
            if image.namespace().as_deref() != namespace {
                return None;
            }
            let kimage_control::api::Source::Build(build) = &image.spec.source else {
                return None;
            };
            build
                .inputs
                .volumes
                .iter()
                .any(|v| {
                    (if secret { &v.secret } else { &v.config_map })
                        .as_ref()
                        .is_some_and(|input| input.name == name)
                })
                .then(|| kube::runtime::reflector::ObjectRef::from_obj(image.as_ref()))
        })
        .collect()
}

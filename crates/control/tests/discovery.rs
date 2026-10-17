//! API and CLI acceptance for read-only discovery and unambiguous mutations.
use axum::{
    Json, Router,
    extract::State,
    http::{Method, StatusCode, Uri},
    response::IntoResponse,
};
use kimage_control::discovery::{self, Target};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Store {
    records: Vec<Value>,
    calls: Vec<String>,
    get_status: Option<u16>,
    list_status: Option<u16>,
    paginate: bool,
}
fn record(namespace: &str, id: &str, manager: &str) -> Value {
    json!({"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":format!("{id}-installation"),"namespace":namespace,"uid":format!("{namespace}-{id}"),"labels":{"kimage.dev/installation":id},"annotations":{"kimage.dev/manager":manager}},"data":{"installation.json":json!({"schema":1,"delivery":"ImagePull","release":"0.5.1","options":{"namespace":namespace,"installation":id,"scope":"cluster","applicationNamespace":null,"excludedNamespaces":[],"runtimeProfile":"inferal","registryNode":"node","workerNodes":{},"exportOrigins":[],"storageClass":null,"storageSize":"1Gi","existingClaim":null,"port":17701}}).to_string()}})
}
async fn fixture(
    store: Store,
) -> (
    kube::Client,
    Arc<Mutex<Store>>,
    String,
    tokio::task::JoinHandle<()>,
) {
    async fn handle(
        State(store): State<Arc<Mutex<Store>>>,
        method: Method,
        uri: Uri,
    ) -> axum::response::Response {
        assert_eq!(
            method,
            Method::GET,
            "Discovery and diagnostics must be read-only"
        );
        let mut store = store.lock().unwrap();
        store.calls.push(uri.to_string());
        let list = uri.path().ends_with("/configmaps");
        let status = if list {
            store.list_status
        } else {
            store.get_status
        };
        if let Some(code) = status {
            return (StatusCode::from_u16(code).unwrap(),Json(json!({"apiVersion":"v1","kind":"Status","status":"Failure","reason":if code==403 {"Forbidden"} else {"Failure"},"message":"fixture API failure","code":code}))).into_response();
        }
        if list {
            let mut items = store.records.clone();
            let prefix = "/api/v1/namespaces/";
            if let Some(path) = uri.path().strip_prefix(prefix) {
                let namespace = path.split('/').next().unwrap();
                items.retain(|item| item["metadata"]["namespace"] == namespace);
            }
            let query: std::collections::HashMap<String, String> =
                reqwest::Url::parse(&format!("http://fixture{uri}"))
                    .unwrap()
                    .query_pairs()
                    .map(|(a, b)| (a.into_owned(), b.into_owned()))
                    .collect();
            if let Some(id) = query
                .get("labelSelector")
                .and_then(|v| v.strip_prefix("kimage.dev/installation="))
            {
                items.retain(|item| item["metadata"]["labels"]["kimage.dev/installation"] == id);
            }
            let continuation = if store.paginate && !query.contains_key("continue") {
                items = items.into_iter().take(1).collect();
                "next"
            } else {
                if store.paginate {
                    items = items.into_iter().skip(1).collect();
                }
                ""
            };
            return Json(json!({"apiVersion":"v1","kind":"ConfigMapList","metadata":{"continue":continuation},"items":items})).into_response();
        }
        if let Some(item) = store.records.iter().find(|r| {
            uri.path()
                == format!(
                    "/api/v1/namespaces/{}/configmaps/{}",
                    r["metadata"]["namespace"].as_str().unwrap(),
                    r["metadata"]["name"].as_str().unwrap()
                )
        }) {
            return Json(item.clone()).into_response();
        }
        (StatusCode::NOT_FOUND,Json(json!({"apiVersion":"v1","kind":"Status","status":"Failure","reason":"NotFound","message":"not found","code":404}))).into_response()
    }
    let store = Arc::new(Mutex::new(store));
    let app = Router::new().fallback(handle).with_state(store.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = kube::Client::try_from(kube::Config::new(url.parse().unwrap())).unwrap();
    (client, store, url, server)
}
#[tokio::test]
async fn discovery_paginates_filters_noise_and_keeps_damaged_records_visible() {
    let mut noise = record("system", "dev", "cli");
    noise["metadata"]["name"] = json!("dev-config");
    let mut damaged = record("a-system", "broken", "helm");
    damaged["data"]["installation.json"] = json!("invalid JSON");
    let (client, store, _, server) = fixture(Store {
        records: vec![record("z-system", "gitops", "argocd"), damaged, noise],
        paginate: true,
        ..Default::default()
    })
    .await;
    let found = discovery::list(client, None, None).await.unwrap();
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].target.installation, "broken");
    assert!(!found[0].valid_record);
    assert_eq!(found[1].manager.as_deref(), Some("argocd"));
    assert_eq!(found[1].scope.as_deref(), Some("cluster"));
    assert!(
        store
            .lock()
            .unwrap()
            .calls
            .iter()
            .any(|c| c.contains("continue=next"))
    );
    server.abort();
}
#[tokio::test]
async fn exact_default_wins_and_explicit_targets_or_opt_out_never_fall_back() {
    let (client, store, _, server) = fixture(Store {
        records: vec![
            record("kimage-system", "kimage", "cli"),
            record("gitops-system", "gitops", "argocd"),
        ],
        ..Default::default()
    })
    .await;
    let selected = discovery::select(client.clone(), None, None, false)
        .await
        .unwrap();
    assert_eq!(
        selected,
        vec![Target {
            namespace: "kimage-system".into(),
            installation: "kimage".into()
        }]
    );
    assert_eq!(store.lock().unwrap().calls.len(), 1);
    for (ns, id, opt_out) in [
        (Some("missing"), Some("missing"), false),
        (None, Some("missing"), true),
    ] {
        let error = discovery::select(client.clone(), ns, id, opt_out)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("InstallationNotFound"));
        assert!(error.contains("kimage installations"));
    }
    assert!(
        !store
            .lock()
            .unwrap()
            .calls
            .iter()
            .any(|c| c.ends_with("configmaps"))
    );
    server.abort();
}
#[tokio::test]
async fn missing_default_discovers_all_and_partial_selectors_constrain_search() {
    let (client, _, _, server) = fixture(Store {
        records: vec![
            record("a-system", "one", "cli"),
            record("b-system", "two", "argocd"),
        ],
        ..Default::default()
    })
    .await;
    let selected = discovery::select(client.clone(), None, None, false)
        .await
        .unwrap();
    assert_eq!(selected.len(), 2);
    let error = discovery::single(selected).unwrap_err().to_string();
    assert!(error.contains("MultipleInstallationsFound"));
    assert!(error.contains("--namespace a-system --installation one"));
    assert!(error.contains("--namespace b-system --installation two"));
    assert_eq!(
        discovery::select(client.clone(), Some("a-system"), None, false)
            .await
            .unwrap()[0]
            .installation,
        "one"
    );
    assert_eq!(
        discovery::select(client, None, Some("two"), false)
            .await
            .unwrap()[0]
            .namespace,
        "b-system"
    );
    server.abort();
}
#[tokio::test]
async fn permission_or_api_failures_are_not_misreported_as_absence() {
    for code in [403, 500] {
        let (client, store, _, server) = fixture(Store {
            get_status: Some(code),
            ..Default::default()
        })
        .await;
        let error = discovery::select(client, None, None, false)
            .await
            .unwrap_err()
            .to_string();
        assert!(!error.contains("NoInstallationsFound"));
        assert_eq!(store.lock().unwrap().calls.len(), 1);
        server.abort();
    }
    let (client, _, _, server) = fixture(Store {
        list_status: Some(403),
        ..Default::default()
    })
    .await;
    let error = discovery::select(client, None, None, false)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("InstallationAccessDenied"));
    assert!(error.contains("--namespace"));
    server.abort();
}
#[tokio::test]
async fn missing_installations_and_disappearing_records_have_actionable_errors() {
    let (client, _, _, server) = fixture(Store::default()).await;
    assert!(
        discovery::select(client.clone(), None, None, false)
            .await
            .unwrap_err()
            .to_string()
            .contains("NoInstallationsFound")
    );
    let error = kimage_control::lifecycle::state(client, "system", "dev")
        .await
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("InstallationNotFound"));
    assert!(!error.contains("ApiError"));
    server.abort();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_checks_every_discovered_installation_and_guards_mutations() {
    let (client, store, url, server) = fixture(Store {
        records: vec![
            record("a-system", "one", "cli"),
            record("b-system", "two", "argocd"),
        ],
        ..Default::default()
    })
    .await;
    let targets = discovery::select(client, None, None, false).await.unwrap();
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("config.json");
    std::fs::write(&config,json!({"apiVersion":"v1","kind":"Config","clusters":[{"name":"fixture","cluster":{"server":url}}],"users":[{"name":"fixture","user":{}}],"contexts":[{"name":"fixture","context":{"cluster":"fixture","user":"fixture"}}],"current-context":"fixture"}).to_string()).unwrap();
    for command in ["status", "doctor"] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_kimage"))
            .args([command, "--json"])
            .env("KUBECONFIG", &config)
            .env_remove("KIMAGE_API_SERVER")
            .env_remove("KIMAGE_TLS_SERVER_NAME")
            .output()
            .unwrap();
        assert!(!output.status.success());
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["installations"].as_array().unwrap().len(), 2);
        for (i, target) in targets.iter().enumerate() {
            assert_eq!(
                value["installations"][i]["installation"],
                target.installation
            );
            assert!(value["installations"][i].get("error").is_some());
        }
    }
    for args in [
        vec!["uninstall"],
        vec!["upgrade", "--to", "0.5.2"],
        vec!["rollback", "--to", "0.5.1"],
    ] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_kimage"))
            .args(args)
            .env("KUBECONFIG", &config)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("MultipleInstallationsFound"));
    }
    for command in ["upgrade", "rollback"] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_kimage"))
            .args([
                command,
                "--to",
                "0.5.2",
                "--namespace",
                "b-system",
                "--installation",
                "two",
            ])
            .env("KUBECONFIG", &config)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("targetRevision"));
    }
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_kimage"))
        .args([
            "uninstall",
            "--namespace",
            "b-system",
            "--installation",
            "two",
        ])
        .env("KUBECONFIG", &config)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("ArgoCDManagedInstallation"));
    assert!(error.contains("--allow-argocd"));
    assert!(
        store
            .lock()
            .unwrap()
            .calls
            .iter()
            .any(|c| c.contains("one-component-images"))
    );
    assert!(
        store
            .lock()
            .unwrap()
            .calls
            .iter()
            .any(|c| c.contains("two-component-images"))
    );
    server.abort();
}

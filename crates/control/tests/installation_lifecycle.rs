//! Stateful API acceptance for interruption/resume and retained identities.
use axum::{Json, Router, http::StatusCode, response::IntoResponse};
use kimage_control::{
    component_images::{Component, ImageCatalog, ROLES},
    install::{InstallScope, RuntimeProfile},
    lifecycle,
    pull_install::{Options, Plan, credentials::Credentials},
};
use kimage_core::oci::Platform;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};
fn catalog(release: &str) -> ImageCatalog {
    ImageCatalog {
        schema: 1,
        release: release.into(),
        api_schema_revision: 5,
        repository: "ghcr.io/inferal-oss/kimage".into(),
        components: ROLES
            .into_iter()
            .map(|role| {
                let version = match role {
                    "builder" => "0.33.0",
                    "registry" => "3.1.2",
                    "gateway" | "proxy" => "0.3.0",
                    _ => release,
                };
                (
                    role.into(),
                    Component {
                        version: version.into(),
                        digest: kimage_core::oci::digest(format!("{role}:{version}").as_bytes()),
                        platforms: BTreeSet::from([
                            Platform::linux_amd64(),
                            Platform::linux_arm64(),
                        ]),
                    },
                )
            })
            .collect(),
    }
}
fn node() -> k8s_openapi::api::core::v1::Node {
    serde_json::from_value(json!({"apiVersion":"v1","kind":"Node","metadata":{"name":"node","uid":"node-uid","labels":{"kubernetes.io/os":"linux","kubernetes.io/arch":"amd64"}},"spec":{},"status":{"conditions":[{"type":"Ready","status":"True"}],"nodeInfo":{"architecture":"amd64","operatingSystem":"linux","kubeletVersion":"v1.36.5+rke2r1","containerRuntimeVersion":"containerd://2.3.4-k3s1","bootID":"boot","kernelVersion":"fixture","machineID":"machine","osImage":"fixture","systemUUID":"system"}}})).unwrap()
}
fn collection(value: &Value) -> String {
    let version = value["apiVersion"].as_str().unwrap();
    let base = if version == "v1" {
        "/api/v1".to_owned()
    } else {
        format!("/apis/{version}")
    };
    let ns = value["metadata"]["namespace"]
        .as_str()
        .map(|n| format!("/namespaces/{n}"))
        .unwrap_or_default();
    let plural = match value["kind"].as_str().unwrap() {
        "PersistentVolumeClaim" => "persistentvolumeclaims",
        "PersistentVolume" => "persistentvolumes",
        "CustomResourceDefinition" => "customresourcedefinitions",
        "NetworkPolicy" => "networkpolicies",
        "MutatingWebhookConfiguration" => "mutatingwebhookconfigurations",
        "ValidatingWebhookConfiguration" => "validatingwebhookconfigurations",
        "ImageRelease" => "imagereleases",
        kind => return format!("{base}{ns}/{}s", kind.to_lowercase()),
    };
    format!("{base}{ns}/{plural}")
}
#[derive(Default)]
struct Store {
    objects: BTreeMap<String, Value>,
    writes: usize,
    fail_after: Option<usize>,
    counter: u64,
}
impl Store {
    fn put(&mut self, mut v: Value) {
        self.counter += 1;
        if v["metadata"]["uid"].is_null() {
            v["metadata"]["uid"] = json!(format!("uid-{}", self.counter));
        }
        v["metadata"]["resourceVersion"] = json!(self.counter.to_string());
        v["metadata"]["generation"] = json!(1);
        match v["kind"].as_str() {
            Some("Pod") => v["status"] = json!({"conditions":[{"type":"Ready","status":"True"}]}),
            Some("Deployment") => {
                v["status"] = json!({"observedGeneration":1,"availableReplicas":v["spec"]["replicas"],"updatedReplicas":v["spec"]["replicas"]})
            }
            Some("Job") => v["status"] = json!({"succeeded":1}),
            _ => {}
        }
        self.objects.insert(
            format!(
                "{}/{}",
                collection(&v),
                v["metadata"]["name"].as_str().unwrap()
            ),
            v,
        );
    }
}
async fn fixture() -> (kube::Client, Arc<Mutex<Store>>, tokio::task::JoinHandle<()>) {
    let mut store = Store::default();
    let credentials = Credentials::generate("system", "dev").unwrap();
    let options = Options {
        namespace: "system".into(),
        installation: "dev".into(),
        scope: InstallScope::Namespace,
        application_namespace: Some("apps".into()),
        excluded_namespaces: BTreeSet::new(),
        runtime_profile: RuntimeProfile::Inferal,
        registry_node: Some("node".into()),
        worker_nodes: BTreeMap::new(),
        export_origins: BTreeSet::new(),
        storage_class: None,
        storage_size: "1Gi".into(),
        existing_claim: None,
        port: 17701,
    };
    let node = node();
    store.put(serde_json::to_value(&node).unwrap());
    let plan = Plan::new(&options, &catalog("0.4.1"), &[node], &credentials).unwrap();
    for mut value in plan.resources.into_iter().chain(plan.webhooks) {
        if value["kind"] == "PersistentVolumeClaim" {
            value["spec"]["volumeName"] = json!("volume");
            value["status"] = json!({"phase":"Bound"});
        }
        if value["metadata"]["name"] == "dev-config" {
            value["data"]["configuration.json"] =
                json!("{ \"schema\": 1, \"buildPolicy\": \"Eager\" }");
        }
        store.put(value);
    }
    store.put(json!({"apiVersion":"v1","kind":"PersistentVolume","metadata":{"name":"volume"},"spec":{"capacity":{"storage":"1Gi"},"accessModes":["ReadWriteOnce"],"persistentVolumeReclaimPolicy":"Retain","local":{"path":"/fixture"}}}));
    for crd in kimage_control::schema::definitions().unwrap() {
        store.put(serde_json::to_value(crd).unwrap());
    }
    let store = Arc::new(Mutex::new(store));
    let records = store.clone();
    let router=Router::new().fallback(move |request:axum::extract::Request| {
        let records=records.clone();async move {
            let method=request.method().to_string();let path=request.uri().path().to_owned();let query=request.uri().query().unwrap_or("").to_owned();
            let body=axum::body::to_bytes(request.into_body(),1024*1024).await.unwrap();
            let mut store=records.lock().unwrap();
            let status=|code:u16,reason:&str|(StatusCode::from_u16(code).unwrap(),Json(json!({"apiVersion":"v1","kind":"Status","status":"Failure","reason":reason,"code":code}))).into_response();
            if method=="GET" {
                if let Some(value)=store.objects.get(&path){return Json(value.clone()).into_response()}
                let collection=path.ends_with("/nodes")||path.ends_with("/pods")||path.ends_with("/jobs")||path.ends_with("/configmaps")||path.ends_with("/imagereleases");
                if collection {
                    let url=reqwest::Url::parse(&format!("http://fixture{path}?{query}")).unwrap();
                    let selector=url.query_pairs().find(|(k,_)|k=="labelSelector").map(|(_,v)|v.into_owned()).unwrap_or_default();
                    let items:Vec<_>=store.objects.iter().filter(|(key,value)|key.rsplit_once('/').unwrap().0==path && selector.split(',').filter(|s|!s.is_empty()).all(|s|s.split_once('=').is_some_and(|(k,v)|value["metadata"]["labels"][k]==v))).map(|(_,v)|v.clone()).collect();
                    return Json(json!({"apiVersion":"v1","kind":"List","metadata":{},"items":items})).into_response()
                }
                return status(404,"NotFound")
            }
            store.writes+=1;
            if store.fail_after==Some(store.writes){store.fail_after=None;return status(500,"InjectedInterruption")}
            if method=="DELETE" {store.objects.remove(&path);return Json(json!({"apiVersion":"v1","kind":"Status","status":"Success"})).into_response()}
            let mut value:Value=serde_json::from_slice(&body).unwrap();
            if value["metadata"]["namespace"].is_null() {let parts:Vec<_>=path.split('/').collect();if let Some(index)=parts.iter().position(|p|*p=="namespaces"){value["metadata"]["namespace"]=json!(parts[index+1]);}}
            if method=="PATCH" {
                let Some(mut current)=store.objects.get(&path).cloned()else{return status(404,"NotFound")};
                if value["metadata"]["uid"]!=current["metadata"]["uid"]||value["metadata"]["resourceVersion"]!=current["metadata"]["resourceVersion"]{return status(409,"Conflict")}
                json_patch::merge(&mut current,&value);value=current;
            }else if method=="POST" && store.objects.contains_key(&format!("{path}/{}",value["metadata"]["name"].as_str().unwrap())) {return status(409,"AlreadyExists")}
            store.put(value.clone());let key=format!("{}/{}",collection(&value),value["metadata"]["name"].as_str().unwrap());
            Json(store.objects[&key].clone()).into_response()
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
    (client, store, server)
}
#[test]
fn compatibility_is_explicit_and_bidirectional_without_implicitly_authorizing_skips() {
    let old = catalog("0.4.1");
    let new = catalog("0.5.0");
    lifecycle::compatible(&old, &new).unwrap();
    lifecycle::compatible(&new, &old).unwrap();
    assert!(lifecycle::compatible(&catalog("0.4.0"), &new).is_err());
    assert!(lifecycle::compatible(&new, &new).is_err());
    let mut incompatible = new;
    incompatible.api_schema_revision = 6;
    assert!(lifecycle::compatible(&old, &incompatible).is_err());
}
#[tokio::test]
async fn read_only_preflight_interrupted_resume_and_reverse_preserve_retained_identities() {
    let (client, store, server) = fixture().await;
    let before = store.lock().unwrap().objects.clone();
    let check = lifecycle::transition(
        client.clone(),
        "system",
        "dev",
        &catalog("0.5.0"),
        true,
        false,
        None,
    )
    .await
    .unwrap();
    assert_eq!(check["checks"], "passed");
    assert_eq!(store.lock().unwrap().writes, 0);
    store.lock().unwrap().fail_after = Some(16);
    assert!(
        lifecycle::transition(
            client.clone(),
            "system",
            "dev",
            &catalog("0.5.0"),
            false,
            false,
            None
        )
        .await
        .is_err()
    );
    let journal = lifecycle::journal(client.clone(), "system", "dev")
        .await
        .unwrap()
        .unwrap();
    assert_ne!(journal.phase, lifecycle::Phase::Complete);
    assert_eq!(
        store.lock().unwrap().objects["/api/v1/namespaces/system/configmaps/dev-operation"]["metadata"]
            ["ownerReferences"][0]["uid"],
        before["/api/v1/namespaces/system/configmaps/dev-installation"]["metadata"]["uid"]
    );
    let resumed = lifecycle::transition(
        client.clone(),
        "system",
        "dev",
        &catalog("0.5.0"),
        false,
        false,
        None,
    )
    .await
    .unwrap();
    assert_eq!(resumed["phase"], "Complete");
    lifecycle::transition(
        client.clone(),
        "system",
        "dev",
        &catalog("0.4.1"),
        false,
        true,
        None,
    )
    .await
    .unwrap();
    let after = &store.lock().unwrap().objects;
    for (key, value) in before.iter().filter(|(k, _)| {
        k.contains("/secrets/")
            || k.contains("/persistentvolume")
            || k.contains("/customresourcedefinitions/")
            || k.ends_with("/dev-installation")
    }) {
        assert_eq!(
            after[key]["metadata"]["uid"], value["metadata"]["uid"],
            "{key}"
        );
        if key.contains("/secrets/") {
            assert_eq!(after[key]["data"], value["data"]);
        }
    }
    assert_eq!(
        after["/api/v1/namespaces/system/configmaps/dev-config"]["data"],
        before["/api/v1/namespaces/system/configmaps/dev-config"]["data"]
    );
    server.abort();
}

#[tokio::test]
async fn incompatible_targets_and_replaced_registry_node_are_rejected_without_writes() {
    let (client, store, server) = fixture().await;
    assert!(
        lifecycle::transition(
            client.clone(),
            "system",
            "dev",
            &catalog("0.6.0"),
            false,
            false,
            None
        )
        .await
        .is_err()
    );
    assert_eq!(store.lock().unwrap().writes, 0);
    store
        .lock()
        .unwrap()
        .objects
        .get_mut("/api/v1/nodes/node")
        .unwrap()["metadata"]["uid"] = json!("replacement-node-uid");
    let error = lifecycle::transition(
        client,
        "system",
        "dev",
        &catalog("0.5.0"),
        false,
        false,
        None,
    )
    .await
    .unwrap_err();
    assert!(error.to_string().starts_with("RegistryNodeIdentityChanged"));
    assert_eq!(store.lock().unwrap().writes, 0);
    server.abort();
}
#[test]
fn native_node_selection_keeps_automatic_placement_and_requires_explicit_pins() {
    let options:Options=serde_json::from_value(json!({"namespace":"system","installation":"dev","scope":"namespace","applicationNamespace":"apps","excludedNamespaces":[],"runtimeProfile":"inferal","registryNode":"node","workerNodes":{"amd64":"node"},"exportOrigins":[],"storageClass":null,"storageSize":"1Gi","existingClaim":null,"port":17701})).unwrap();
    let original = node();
    let mut added = original.clone();
    added.metadata.name = Some("a-new-node".into());
    added.metadata.uid = Some("new-uid".into());
    let mut arm = original.clone();
    arm.metadata.name = Some("arm".into());
    arm.metadata.uid = Some("arm-uid".into());
    arm.metadata
        .labels
        .as_mut()
        .unwrap()
        .insert("kubernetes.io/arch".into(), "arm64".into());
    arm.status
        .as_mut()
        .unwrap()
        .node_info
        .as_mut()
        .unwrap()
        .architecture = "arm64".into();
    let mut policy = kimage_control::node_reconcile::Policy::default();
    let (_, workers) = kimage_control::node_reconcile::select(
        &options,
        &policy,
        &[original.clone(), added.clone(), arm.clone()],
    )
    .unwrap();
    assert_eq!(workers["amd64"], "node");
    assert_eq!(workers["arm64"], "arm");
    policy
        .worker_pins
        .insert("amd64".into(), "a-new-node".into());
    let (_, workers) = kimage_control::node_reconcile::select(
        &options,
        &policy,
        &[original.clone(), added, arm.clone()],
    )
    .unwrap();
    assert_eq!(workers["amd64"], "a-new-node");
    assert!(
        kimage_control::node_reconcile::select(&options, &policy, &[original.clone(), arm])
            .is_err()
    );
    policy.worker_pins.clear();
    policy.node_selector.insert("fixture".into(), "yes".into());
    assert!(kimage_control::node_reconcile::select(&options, &policy, &[original]).is_err());
}

#[tokio::test]
async fn an_older_operator_can_ignore_a_completed_newer_journal_but_fences_unknown_active_formats()
{
    let (client, store, server) = fixture().await;
    let make = |phase: &str| json!({"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":"dev-operation","namespace":"system","labels":{"kimage.dev/installation":"dev"}},"data":{"operation.json":json!({"schema":99,"source":{"release":"0.6.0"},"target":{"release":"0.5.0"},"phase":phase,"futureField":true}).to_string()}});
    store.lock().unwrap().put(make("Complete"));
    assert!(
        !lifecycle::active(client.clone(), "system", "dev")
            .await
            .unwrap()
    );
    store.lock().unwrap().put(make("UnknownFuturePhase"));
    assert!(lifecycle::active(client, "system", "dev").await.unwrap());
    assert_eq!(store.lock().unwrap().writes, 0);
    server.abort();
}

#[tokio::test]
async fn node_replacement_revokes_new_builds_before_waiting_for_an_active_worker_lease() {
    let (client, store, server) = fixture().await;
    lifecycle::transition(
        client.clone(),
        "system",
        "dev",
        &catalog("0.5.0"),
        false,
        false,
        None,
    )
    .await
    .unwrap();
    let mut second = node();
    second.metadata.name = Some("second".into());
    second.metadata.uid = Some("second-node-uid".into());
    let old_uid = store.lock().unwrap().objects["/api/v1/namespaces/system/pods/dev-builder-amd64"]
        ["metadata"]["uid"]
        .clone();
    {
        let mut records = store.lock().unwrap();
        records.put(serde_json::to_value(second).unwrap());
        records.put(json!({"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":"dev-node-policy","namespace":"system","labels":{"kimage.dev/installation":"dev"},"annotations":{"kimage.dev/manager":"cli"}},"data":{"policy.json":json!({"schema":1,"nodeSelector":{},"workerPins":{"amd64":"second"}}).to_string()}}));
        records.put(json!({"apiVersion":"coordination.k8s.io/v1","kind":"Lease","metadata":{"name":"dev-direct-worker-amd64","namespace":"system","labels":{"kimage.dev/installation":"dev"}},"spec":{"holderIdentity":format!("{}:active","a".repeat(64)),"renewTime":k8s_openapi::jiff::Timestamp::now().to_string(),"leaseDurationSeconds":30}}));
    }
    let config = Arc::new(kimage_control::reconcile::Config {
        scope: Some(kimage_core::scope::NamespaceScope {
            system_namespace: "system".into(),
            application_namespace: Some("apps".into()),
            cluster_wide: false,
            excluded_namespaces: BTreeSet::new(),
        }),
        direct: Some(kimage_control::direct::Settings {
            architecture: None,
            system_namespace: "system".into(),
            installation: "dev".into(),
            builder_image: catalog("0.5.0").reference("builder").unwrap(),
        }),
        client: client.clone(),
        namespace: "apps".into(),
        node: "node".into(),
        input_image: catalog("0.5.0").reference("input").unwrap(),
        registry_origin: "https://dev-registry.system.svc:5443/".parse().unwrap(),
        registry_ca: "unused".into(),
        publisher_password: "unused".into(),
        registry_pod: None,
        cancel: tokio_util::sync::CancellationToken::new(),
        concurrency: Arc::new(tokio::sync::Semaphore::new(2)),
    });
    let error = kimage_control::node_reconcile::once(config)
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "BuilderBusy");
    let records = store.lock().unwrap();
    assert!(
        !records
            .objects
            .contains_key("/api/v1/namespaces/system/configmaps/dev-builders-amd64")
    );
    assert_eq!(
        records.objects["/api/v1/namespaces/system/pods/dev-builder-amd64"]["metadata"]["uid"],
        old_uid
    );
    assert!(records.objects["/api/v1/namespaces/system/configmaps/dev-node-status"]["data"]["status.json"].as_str().unwrap().contains("BuilderBusy"));
    server.abort();
}

#[tokio::test]
async fn doctor_reports_actionable_drift_and_certificate_health_without_writes_or_private_material()
{
    let (client, store, server) = fixture().await;
    lifecycle::transition(
        client.clone(),
        "system",
        "dev",
        &catalog("0.5.0"),
        false,
        false,
        None,
    )
    .await
    .unwrap();
    {
        let mut records = store.lock().unwrap();
        let map = records
            .objects
            .get_mut("/api/v1/namespaces/system/configmaps/dev-builders-amd64")
            .unwrap();
        let mut data: Value =
            serde_json::from_str(map["data"]["catalog.json"].as_str().unwrap()).unwrap();
        data["builders"]["default"]["inputImage"] = json!("unexpected");
        map["data"]["catalog.json"] = json!(serde_json::to_string(&data).unwrap());
    }
    let writes = store.lock().unwrap().writes;
    let report = kimage_control::diagnostics::inspect(client, "system", "dev", true)
        .await
        .unwrap();
    assert_eq!(store.lock().unwrap().writes, writes);
    assert_eq!(report["release"], "0.5.0");
    assert_eq!(report["healthy"], false);
    assert!(
        report["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["reason"] == "InstallationResourceDrift"
                && i["resource"] == "ConfigMap/dev-builders-amd64")
    );
    assert!(
        report["certificates"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["validNow"] == true && c["validForThirtyDays"] == true)
    );
    let output = serde_json::to_string(&report).unwrap();
    for secret in store
        .lock()
        .unwrap()
        .objects
        .values()
        .filter(|v| v["kind"] == "Secret")
    {
        for material in secret["data"].as_object().unwrap().values() {
            let text = material.as_str().unwrap();
            if text.len() > 32 {
                assert!(!output.contains(text));
            }
        }
    }
    server.abort();
}

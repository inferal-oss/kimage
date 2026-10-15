//! Regression for the live completed-journal/missing-client cleanup failure.
use axum::{Json, Router, http::StatusCode, response::IntoResponse};
use base64::{Engine, engine::general_purpose::STANDARD};
use kimage_control::{api::Image, reconcile::Config};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

struct ApiState {
    image: Value,
    secret: Value,
    release: Value,
    requests: Vec<String>,
}

async fn recover(trusted_root: bool) {
    let id = "a".repeat(64);
    let ordinary = kimage_core::oci::digest(b"new ordinary input");
    let descriptor = |bytes: &[u8]| json!({"mediaType":kimage_core::oci::MANIFEST,"digest":kimage_core::oci::digest(bytes),"size":bytes.len()});
    let old_root = descriptor(b"old");
    let new_root = descriptor(b"new");
    let build = json!({"format":"Containerfile","definition":{"inline":"FROM scratch\nCOPY message /message\n"},"execution":{"strategy":"Direct","builderRef":"default"},"inputs":{"contextPath":"/workspace/context","volumes":[{"name":"source","configMap":{"name":"source"}}],"volumeMounts":[{"name":"source","mountPath":"/workspace/context","readOnly":true}]}});
    let mut image = json!({"apiVersion":"kimage.dev/v1alpha1","kind":"Image","metadata":{"name":"image","namespace":"app","uid":"image-uid","generation":1,"resourceVersion":"1"},"spec":{"version":"1.0.0","source":{"build":build},"platform":{"os":"linux","architecture":"amd64"}},"status":{"observedGeneration":1,"conditions":[],"build":{"snapshotId":id,"ordinaryDigest":ordinary,"privateStateRef":"placeholder"},"operation":{"identity":{"imageUid":"image-uid","generation":1,"version":"1.0.0","snapshot":ordinary},"attempt":2,"phase":"Publishing","root":if trusted_root{new_root.clone()}else{Value::Null}}}});
    let typed: Image = serde_json::from_value(image.clone()).unwrap();
    let state_name = kimage_control::build_state::name(&typed).unwrap();
    image["status"]["build"]["privateStateRef"] = json!(state_name);
    let mut source = serde_json::to_value(&typed.spec.source).unwrap();
    source["build"].as_object_mut().unwrap().remove("execution");
    source["build"]["cache"]
        .as_object_mut()
        .unwrap()
        .remove("mode");
    let request = kimage_core::oci::digest(
        &serde_json::to_vec(&json!({"source":source,"platform":typed.spec.platform})).unwrap(),
    );
    let reference =
        |rv| json!({"kind":"ConfigMap","name":"source","uid":"source-uid","resourceVersion":rv});
    let state = json!({"imageUid":"image-uid","version":"1.0.0","generation":1,"snapshotId":id,"requestDigest":request,"fingerprint":"new","frozen":{"build":build,"references":[reference("2")],"bindings":[]},"capture":{"ordinaryDigest":ordinary,"definitionSensitive":false,"definitionDigest":kimage_core::oci::digest(b"recipe"),"files":1,"bytes":3},"workerUid":"old-worker","phase":"Completed","root":new_root,"execution":null,"publishedFingerprint":"old","publishedReferences":[reference("1")],"failedReason":null});
    let secret = json!({"apiVersion":"v1","kind":"Secret","metadata":{"name":state_name,"uid":"state-uid","resourceVersion":"1","labels":{"kimage.dev/build-state":"image-uid"},"ownerReferences":[{"apiVersion":"kimage.dev/v1alpha1","kind":"Image","name":"image","uid":"image-uid"}]},"data":{"state.json":STANDARD.encode(serde_json::to_vec(&state).unwrap())}});
    let release = json!({"apiVersion":"kimage.dev/v1alpha1","kind":"ImageRelease","metadata":{"name":kimage_core::catalog::release_name("image-uid","1.0.0"),"uid":"retained-uid","generation":1},"spec":{"imageName":"image","imageUid":"image-uid","version":"1.0.0","snapshot":kimage_core::oci::digest(b"old ordinary input"),"root":old_root,"platform":{"os":"linux","architecture":"amd64"},"requestDigest":request,"sourceGeneration":1,"publisherVersion":"0.1.0"},"status":{"verified":true,"observedGeneration":1,"conditions":[]}});
    let shared = Arc::new(Mutex::new(ApiState {
        image,
        secret,
        release,
        requests: Vec::new(),
    }));
    let api = shared.clone();
    let router = Router::new().fallback(move |request: axum::extract::Request| {
        let api = api.clone();
        async move {
            let path = request.uri().path().to_owned();
            let method = request.method().to_string();
            let bytes = axum::body::to_bytes(request.into_body(), 1_048_576).await.unwrap();
            let mut state = api.lock().unwrap();
            state.requests.push(format!("{method} {path}"));
            let value = match (method.as_str(), path.as_str()) {
                ("GET", "/apis/kimage.dev/v1alpha1/namespaces/app/images/image") => state.image.clone(),
                ("GET", p) if p.contains("/imagereleases/") => state.release.clone(),
                ("GET", "/api/v1/namespaces/app/configmaps/source") => json!({"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":"source","uid":"source-uid","resourceVersion":"2"}}),
                ("GET", "/api/v1/namespaces/app/configmaps") => json!({"apiVersion":"v1","kind":"ConfigMapList","metadata":{},"items":[]}),
                ("GET", "/api/v1/namespaces/app/secrets") => json!({"apiVersion":"v1","kind":"SecretList","metadata":{},"items":[]}),
                ("GET", p) if p.contains("/secrets/buildstate-") => state.secret.clone(),
                ("PATCH", p) if p.contains("/secrets/buildstate-") => {
                    state.secret["data"] = serde_json::from_slice::<Value>(&bytes).unwrap()["data"].clone();
                    state.secret.clone()
                }
                ("PATCH", "/apis/kimage.dev/v1alpha1/namespaces/app/images/image/status") => {
                    state.image["status"] = serde_json::from_slice::<Value>(&bytes).unwrap()["status"].clone();
                    state.image.clone()
                }
                ("GET", p) if p.contains("/pods/build-") => return (StatusCode::NOT_FOUND, Json(json!({"apiVersion":"v1","kind":"Status","status":"Failure","reason":"NotFound","message":"client already removed","code":404}))).into_response(),
                _ => panic!("unexpected recovery API call: {method} {path}"),
            };
            Json(value).into_response()
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
    let config = Arc::new(Config {
        scope: None,
        client,
        namespace: "app".into(),
        node: "node".into(),
        input_image: "unused".into(),
        registry_origin: "https://unused.invalid/".parse().unwrap(),
        registry_ca: "missing-ca".into(),
        publisher_password: "missing-password".into(),
        registry_pod: None,
        cancel: tokio_util::sync::CancellationToken::new(),
        concurrency: Arc::new(tokio::sync::Semaphore::new(1)),
        direct: Some(kimage_control::direct::Settings {
            architecture: None,
            system_namespace: "system".into(),
            installation: "fixture".into(),
            builder_image: "unused".into(),
        }),
    });
    let image = serde_json::from_value(shared.lock().unwrap().image.clone()).unwrap();
    kimage_control::build_reconcile::reconcile(Arc::new(image), config)
        .await
        .unwrap();
    let result = shared.lock().unwrap();
    let expected = if trusted_root {
        "VersionConflict"
    } else {
        "BuildExportUnavailable"
    };
    assert_eq!(result.image["status"]["conditions"][0]["reason"], expected);
    assert_eq!(result.image["status"]["conditions"][0]["status"], "False");
    assert_eq!(result.release["metadata"]["uid"], "retained-uid");
    assert_eq!(result.release["spec"]["root"], old_root);
    let saved: Value = serde_json::from_slice(
        &STANDARD
            .decode(result.secret["data"]["state.json"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap();
    assert_eq!(saved["phase"], "Failed");
    assert_eq!(saved["failedReason"], expected);
    assert!(
        !result
            .requests
            .iter()
            .any(|r| r.contains("/system/") || r.starts_with("POST "))
    );
    server.abort();
}

#[tokio::test]
async fn completed_missing_client_reports_conflict_without_engine_or_source_rebuild() {
    recover(true).await;
}

#[tokio::test]
async fn private_journal_root_cannot_replace_the_public_verified_root_checkpoint() {
    recover(false).await;
}

//! Exclusive renewable coordination for native serving transitions.
use crate::install::LABEL;
use anyhow::{Context, Result, ensure};
use k8s_openapi::{
    api::coordination::v1::{Lease, LeaseSpec},
    apimachinery::pkg::apis::meta::v1::{MicroTime, ObjectMeta},
    jiff::Timestamp,
};
use kube::{Api, ResourceExt, api::PostParams};
use std::{collections::BTreeMap, time::Duration};
use tokio_util::sync::CancellationToken;
pub struct OperationLease {
    pub holder: String,
    api: Api<Lease>,
    name: String,
    stop: CancellationToken,
    pub lost: CancellationToken,
}
impl OperationLease {
    pub async fn acquire(
        client: kube::Client,
        namespace: &str,
        installation: &str,
        name: &str,
        holder: &str,
    ) -> Result<Self> {
        ensure!(
            holder.len() == 64 && holder.bytes().all(|c| c.is_ascii_hexdigit()),
            "InvalidOperationIdentity"
        );
        let api: Api<Lease> = Api::namespaced(client, namespace);
        let name = name.to_owned();
        let mut instance = [0_u8; 16];
        getrandom::fill(&mut instance)
            .map_err(|_| anyhow::anyhow!("OperationIdentityUnavailable"))?;
        let holder = format!("{holder}:{}", hex::encode(instance));
        claim(&api, installation, &name, &holder).await?;
        let result = Self {
            holder,
            api,
            name,
            stop: CancellationToken::new(),
            lost: CancellationToken::new(),
        };
        let stop = result.stop.clone();
        let lost = result.lost.clone();
        let api = result.api.clone();
        let name = result.name.clone();
        let holder = result.holder.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {_=stop.cancelled()=>break,_=tokio::time::sleep(Duration::from_secs(5))=>{}}
                if renew(&api, &name, &holder).await.is_err() {
                    lost.cancel();
                    break;
                }
            }
        });
        Ok(result)
    }
    pub async fn acquire_wait(
        client: kube::Client,
        namespace: &str,
        installation: &str,
        name: &str,
        holder: &str,
    ) -> Result<Self> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(35);
        loop {
            match Self::acquire(client.clone(), namespace, installation, name, holder).await {
                Ok(lease) => return Ok(lease),
                Err(error)
                    if matches!(
                        error.to_string().as_str(),
                        "OperationBusy" | "OperationLeaseAcquireConflict"
                    ) && tokio::time::Instant::now() < deadline =>
                {
                    tokio::time::sleep(Duration::from_millis(250)).await
                }
                Err(error) => return Err(error),
            }
        }
    }
    pub async fn check(&self) -> Result<()> {
        ensure!(!self.lost.is_cancelled(), "OperationLeaseLost");
        renew(&self.api, &self.name, &self.holder).await
    }
    pub async fn release(&self) -> Result<()> {
        self.stop.cancel();
        for attempt in 0..3 {
            let mut current = self.api.get(&self.name).await?;
            if current
                .spec
                .as_ref()
                .and_then(|s| s.holder_identity.as_deref())
                != Some(&self.holder)
            {
                return Ok(());
            }
            let spec = current.spec.as_mut().context("MissingOperationLeaseSpec")?;
            spec.holder_identity = None;
            spec.renew_time = None;
            match self
                .api
                .replace(&self.name, &PostParams::default(), &current)
                .await
            {
                Ok(_) => return Ok(()),
                Err(kube::Error::Api(error)) if error.code == 409 && attempt < 2 => continue,
                Err(error) => return Err(error.into()),
            }
        }
        anyhow::bail!("OperationLeaseReleaseConflict")
    }
}
impl Drop for OperationLease {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}
// A competing acquire or release may change resourceVersion after our GET.
// Every retry reads and validates ownership and expiry again before claiming.
async fn claim(api: &Api<Lease>, installation: &str, name: &str, holder: &str) -> Result<()> {
    for _ in 0..3 {
        let existing = api.get_opt(name).await?;
        let now = Timestamp::now();
        if let Some(current) = &existing {
            ensure!(
                current
                    .labels()
                    .get(LABEL)
                    .is_some_and(|v| v == installation),
                "OperationLeaseOwnershipConflict"
            );
            if let Some(spec) = &current.spec {
                let expiry = spec.renew_time.as_ref().map(|t| {
                    t.0.as_second() + i64::from(spec.lease_duration_seconds.unwrap_or(30))
                });
                if let Some(previous) = &spec.holder_identity {
                    ensure!(
                        expiry.is_some_and(|t| t <= now.as_second()),
                        "OperationBusy"
                    );
                    let previous_id = previous.split(':').next().unwrap_or("");
                    ensure!(
                        previous_id.len() == 64
                            && previous_id.bytes().all(|c| c.is_ascii_hexdigit()),
                        "InvalidPreviousOperationLease"
                    );
                }
            }
        }
        let mut lease = existing.clone().unwrap_or_else(|| Lease {
            metadata: ObjectMeta {
                name: Some(name.into()),
                labels: Some(BTreeMap::from([(LABEL.into(), installation.to_owned())])),
                ..Default::default()
            },
            spec: None,
        });
        lease.spec = Some(LeaseSpec {
            holder_identity: Some(holder.into()),
            renew_time: Some(MicroTime(now)),
            lease_duration_seconds: Some(30),
            ..Default::default()
        });
        let result = if existing.is_some() {
            api.replace(name, &PostParams::default(), &lease).await
        } else {
            api.create(&PostParams::default(), &lease).await
        };
        match result {
            Ok(_) => return Ok(()),
            Err(kube::Error::Api(error)) if error.code == 409 => continue,
            Err(error) => return Err(error.into()),
        }
    }
    anyhow::bail!("OperationLeaseAcquireConflict")
}
async fn renew(api: &Api<Lease>, name: &str, holder: &str) -> Result<()> {
    let started = tokio::time::Instant::now();
    for attempt in 0..3 {
        let mut current = api.get(name).await?;
        ensure!(
            current
                .spec
                .as_ref()
                .and_then(|s| s.holder_identity.as_deref())
                == Some(holder),
            "OperationLeaseLost"
        );
        current
            .spec
            .as_mut()
            .context("MissingOperationLeaseSpec")?
            .renew_time = Some(MicroTime(Timestamp::now()));
        match api.replace(name, &PostParams::default(), &current).await {
            Ok(_) => {
                ensure!(
                    started.elapsed() < Duration::from_secs(10),
                    "OperationLeaseRenewExpired"
                );
                return Ok(());
            }
            Err(kube::Error::Api(error)) if error.code == 409 && attempt < 2 => continue,
            Err(error) => return Err(error.into()),
        }
    }
    anyhow::bail!("OperationLeaseRenewConflict")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router, body::to_bytes, extract::Request, http::StatusCode, response::IntoResponse,
    };
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Copy)]
    enum Race {
        Released,
        Busy,
        Foreign,
        Repeated,
        Created,
    }
    struct Store {
        value: Option<Value>,
        race: Race,
        writes: usize,
        claims: usize,
    }
    fn stored(holder: Option<String>) -> Value {
        json!({"apiVersion":"coordination.k8s.io/v1","kind":"Lease","metadata":{"name":"dev-lifecycle","namespace":"system","uid":"lease-uid","resourceVersion":"1","labels":{"kimage.dev/installation":"dev"}},"spec":{"holderIdentity":holder,"renewTime":Timestamp::now().to_string(),"leaseDurationSeconds":30}})
    }
    fn failure(code: StatusCode, reason: &str) -> axum::response::Response {
        (code,Json(json!({"apiVersion":"v1","kind":"Status","status":"Failure","code":code.as_u16(),"reason":reason,"message":reason}))).into_response()
    }
    async fn fixture(race: Race) -> (Api<Lease>, Arc<Mutex<Store>>, tokio::task::JoinHandle<()>) {
        let store = Arc::new(Mutex::new(Store {
            value: if matches!(race, Race::Created) {
                None
            } else {
                Some(stored(None))
            },
            race,
            writes: 0,
            claims: 0,
        }));
        let shared = store.clone();
        let router = Router::new().fallback(axum::routing::any(move |request: Request| {
            let shared = shared.clone();
            async move {
                let method = request.method().clone();
                let bytes = to_bytes(request.into_body(), 16384).await.unwrap();
                let mut state = shared.lock().unwrap();
                if method == axum::http::Method::GET {
                    return state
                        .value
                        .clone()
                        .map(|v| Json(v).into_response())
                        .unwrap_or_else(|| failure(StatusCode::NOT_FOUND, "NotFound"));
                }
                assert!(matches!(
                    method,
                    axum::http::Method::PUT | axum::http::Method::POST
                ));
                let value: Value = serde_json::from_slice(&bytes).unwrap();
                if let Some(old) = &state.value {
                    assert_eq!(value["metadata"]["uid"], old["metadata"]["uid"]);
                    assert_eq!(
                        value["metadata"]["resourceVersion"],
                        old["metadata"]["resourceVersion"]
                    );
                }
                state.writes += 1;
                if state.writes == 1 || matches!(state.race, Race::Repeated) {
                    let mut current = stored(None);
                    current["metadata"]["resourceVersion"] = json!((state.writes + 1).to_string());
                    match state.race {
                        Race::Busy | Race::Created => {
                            current["spec"]["holderIdentity"] =
                                json!(format!("{}:competitor", "b".repeat(64)))
                        }
                        Race::Foreign => current["metadata"]["labels"][LABEL] = json!("other"),
                        _ => {}
                    }
                    state.value = Some(current);
                    return failure(StatusCode::CONFLICT, "Conflict");
                }
                state.claims += 1;
                state.value = Some(value.clone());
                Json(value).into_response()
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let client = kube::Client::try_from(kube::Config::new(
            format!("http://{address}").parse().unwrap(),
        ))
        .unwrap();
        (Api::namespaced(client, "system"), store, server)
    }
    #[tokio::test]
    async fn acquisition_retries_a_release_race_with_fresh_resource_version() {
        let (api, store, server) = fixture(Race::Released).await;
        claim(&api, "dev", "dev-lifecycle", "claimed")
            .await
            .unwrap();
        let state = store.lock().unwrap();
        assert_eq!(state.writes, 2);
        assert_eq!(state.claims, 1);
        assert_eq!(
            state.value.as_ref().unwrap()["spec"]["holderIdentity"],
            "claimed"
        );
        server.abort();
    }
    #[tokio::test]
    async fn acquisition_rechecks_active_holder_and_ownership_after_conflicts() {
        for (race, reason) in [
            (Race::Busy, "OperationBusy"),
            (Race::Created, "OperationBusy"),
            (Race::Foreign, "OperationLeaseOwnershipConflict"),
        ] {
            let (api, store, server) = fixture(race).await;
            assert_eq!(
                claim(&api, "dev", "dev-lifecycle", "claimed")
                    .await
                    .unwrap_err()
                    .to_string(),
                reason
            );
            let state = store.lock().unwrap();
            assert_eq!(state.writes, 1);
            assert_eq!(state.claims, 0);
            server.abort();
        }
    }
    #[tokio::test]
    async fn acquisition_conflicts_are_bounded_without_overwriting_a_competitor() {
        let (api, store, server) = fixture(Race::Repeated).await;
        assert_eq!(
            claim(&api, "dev", "dev-lifecycle", "claimed")
                .await
                .unwrap_err()
                .to_string(),
            "OperationLeaseAcquireConflict"
        );
        let state = store.lock().unwrap();
        assert_eq!(state.writes, 3);
        assert_eq!(state.claims, 0);
        server.abort();
    }
}

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
        let existing = api.get_opt(&name).await?;
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
        let mut instance = [0_u8; 16];
        getrandom::fill(&mut instance)
            .map_err(|_| anyhow::anyhow!("OperationIdentityUnavailable"))?;
        let holder = format!("{holder}:{}", hex::encode(instance));
        let mut lease = existing.clone().unwrap_or_else(|| Lease {
            metadata: ObjectMeta {
                name: Some(name.clone()),
                labels: Some(BTreeMap::from([(LABEL.into(), installation.to_owned())])),
                ..Default::default()
            },
            spec: None,
        });
        lease.spec = Some(LeaseSpec {
            holder_identity: Some(holder.clone()),
            renew_time: Some(MicroTime(now)),
            lease_duration_seconds: Some(30),
            ..Default::default()
        });
        if existing.is_some() {
            api.replace(&name, &PostParams::default(), &lease).await?;
        } else {
            api.create(&PostParams::default(), &lease).await?;
        }
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

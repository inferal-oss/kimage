//! Administrator policy and bounded, non-sensitive per-Image platform demand.
use crate::{
    api::Image,
    install::{Installer, LABEL},
};
use anyhow::{Context, Result, ensure};
use k8s_openapi::api::core::v1::ConfigMap;
use kimage_core::oci::{self, Platform};
use kube::{Api, Client, ResourceExt, api::PostParams};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum BuildPolicy {
    #[default]
    OnDemand,
    Eager,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Configuration {
    pub schema: u32,
    #[serde(default)]
    pub build_policy: BuildPolicy,
}
impl Default for Configuration {
    fn default() -> Self {
        Self {
            schema: 1,
            build_policy: BuildPolicy::OnDemand,
        }
    }
}
/// Explicit platforms are an administrator-visible build request. Automatic
/// Images remain idle by default until actual workload placement creates demand.
pub fn plan(
    spec: &crate::api::ImageSpec,
    policy: BuildPolicy,
    available: &BTreeSet<Platform>,
    demand: &BTreeSet<Platform>,
) -> BTreeSet<Platform> {
    let requested = spec.explicit_platforms().unwrap_or_else(|| match policy {
        BuildPolicy::OnDemand => demand.clone(),
        BuildPolicy::Eager => available.clone(),
    });
    requested.intersection(available).cloned().collect()
}
pub(crate) fn parse(map: &ConfigMap, installation: &str) -> Result<Configuration> {
    ensure!(
        map.labels().get(LABEL).is_some_and(|v| v == installation),
        "ConfigurationOwnershipConflict"
    );
    let config: Configuration = serde_json::from_str(
        map.data
            .as_ref()
            .and_then(|d| d.get("configuration.json"))
            .context("MissingInstallationConfiguration")?,
    )?;
    ensure!(config.schema == 1, "UnsupportedInstallationConfiguration");
    Ok(config)
}
pub async fn load(client: Client, system: &str, installation: &str) -> Result<Configuration> {
    let maps: Api<ConfigMap> = Api::namespaced(client, system);
    match maps.get_opt(&format!("{installation}-config")).await? {
        Some(map) => parse(&map, installation),
        None => Ok(Configuration::default()),
    }
}
impl Installer {
    pub(crate) async fn ensure_configuration(&self) -> Result<()> {
        let maps: Api<ConfigMap> = Api::namespaced(self.client.clone(), &self.options.namespace);
        let name = self.name("config");
        if let Some(map) = maps.get_opt(&name).await? {
            parse(&map, &self.options.installation)?;
            return Ok(());
        }
        let map: ConfigMap = serde_json::from_value(
            serde_json::json!({"apiVersion":"v1","kind":"ConfigMap","metadata":self.metadata(&name),"data":{"configuration.json":serde_json::to_string(&Configuration::default())?}}),
        )?;
        match maps.create(&PostParams::default(), &map).await {
            Ok(_) => Ok(()),
            Err(kube::Error::Api(e)) if e.code == 409 => {
                parse(&maps.get(&name).await?, &self.options.installation)?;
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }
}
#[derive(
    kube::CustomResource, schemars::JsonSchema, Clone, Debug, Serialize, Deserialize, PartialEq, Eq,
)]
#[kube(
    group = "kimage.dev",
    version = "v1alpha1",
    kind = "ImageDemand",
    namespaced
)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Demand {
    pub schema: u32,
    pub namespace: String,
    pub image_name: String,
    pub image_uid: String,
    pub generation: i64,
    pub version: String,
    pub platforms: BTreeSet<Platform>,
}
impl Demand {
    pub fn for_image(image: &Image, platforms: BTreeSet<Platform>) -> Result<Self> {
        ensure!(
            !platforms.is_empty()
                && platforms.len() <= 2
                && platforms.iter().all(Platform::supported),
            "InvalidPlatformDemand"
        );
        Ok(Self {
            schema: 1,
            namespace: image.namespace().context("MissingImageNamespace")?,
            image_name: image.name_any(),
            image_uid: image.uid().context("MissingImageUid")?,
            generation: image.metadata.generation.unwrap_or(1),
            version: image.spec.version.clone(),
            platforms,
        })
    }
    pub fn name(&self, installation: &str) -> String {
        format!(
            "{installation}-demand-{}",
            &oci::digest(self.image_uid.as_bytes())[7..27]
        )
    }
    fn merge(&mut self, incoming: &Self) -> Result<()> {
        ensure!(
            self.schema == 1
                && self.namespace == incoming.namespace
                && self.image_name == incoming.image_name
                && self.image_uid == incoming.image_uid,
            "PlatformDemandIdentityConflict"
        );
        ensure!(
            self.generation <= incoming.generation,
            "StalePlatformDemand"
        );
        if self.generation == incoming.generation {
            ensure!(
                self.version == incoming.version,
                "PlatformDemandVersionConflict"
            );
            self.platforms.extend(incoming.platforms.clone());
        } else {
            *self = incoming.clone();
        }
        ensure!(
            self.platforms.len() <= 2 && self.platforms.iter().all(Platform::supported),
            "InvalidPlatformDemand"
        );
        Ok(())
    }
}
fn demand_from(map: &ImageDemand, installation: &str) -> Result<Demand> {
    ensure!(
        map.labels().get(LABEL).is_some_and(|v| v == installation)
            && map
                .labels()
                .get("kimage.dev/role")
                .is_some_and(|v| v == "platform-demand"),
        "PlatformDemandOwnershipConflict"
    );
    Ok(map.spec.clone())
}
pub async fn record(
    client: Client,
    system: &str,
    installation: &str,
    demand: Demand,
) -> Result<()> {
    let maps: Api<ImageDemand> = Api::namespaced(client, system);
    let name = demand.name(installation);
    for _ in 0..3 {
        let previous = maps.get_opt(&name).await?;
        let mut merged = demand.clone();
        if let Some(map) = &previous {
            merged = demand_from(map, installation)?;
            let old = merged.clone();
            merged.merge(&demand)?;
            if old == merged {
                return Ok(());
            }
        }
        let mut map = previous.clone().unwrap_or_else(|| {
            let mut resource = ImageDemand::new(&name, merged.clone());
            resource.metadata.labels = Some(BTreeMap::from([
                (LABEL.into(), installation.into()),
                ("kimage.dev/role".into(), "platform-demand".into()),
            ]));
            resource
        });
        map.spec = merged;
        let result = if previous.is_some() {
            maps.replace(&name, &PostParams::default(), &map).await
        } else {
            maps.create(&PostParams::default(), &map).await
        };
        match result {
            Ok(_) => return Ok(()),
            Err(kube::Error::Api(e)) if e.code == 409 => continue,
            Err(e) => return Err(e.into()),
        }
    }
    anyhow::bail!("PlatformDemandChangedRetry")
}
pub async fn requested(
    client: Client,
    system: &str,
    installation: &str,
    image: &Image,
) -> Result<BTreeSet<Platform>> {
    let desired = Demand::for_image(image, BTreeSet::from([Platform::linux_amd64()]))?;
    let maps: Api<ImageDemand> = Api::namespaced(client, system);
    let Some(map) = maps.get_opt(&desired.name(installation)).await? else {
        return Ok(BTreeSet::new());
    };
    let recorded = demand_from(&map, installation)?;
    ensure!(
        recorded.image_uid == desired.image_uid
            && recorded.namespace == desired.namespace
            && recorded.image_name == desired.image_name,
        "PlatformDemandIdentityConflict"
    );
    if recorded.generation != desired.generation || recorded.version != desired.version {
        return Ok(BTreeSet::new());
    }
    ensure!(
        recorded.schema == 1
            && recorded.platforms.len() <= 2
            && recorded.platforms.iter().all(Platform::supported),
        "InvalidPlatformDemand"
    );
    Ok(recorded.platforms)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_to_demand_and_generation_scoped_requests_are_bounded_and_monotonic() {
        assert_eq!(Configuration::default().build_policy, BuildPolicy::OnDemand);
        assert!(
            serde_json::from_str::<Configuration>(r#"{"schema":1,"buildPolicy":"Unknown"}"#)
                .is_err()
        );
        let mut first = Demand {
            schema: 1,
            namespace: "app".into(),
            image_name: "service".into(),
            image_uid: "uid".into(),
            generation: 1,
            version: "1.0.0".into(),
            platforms: BTreeSet::from([Platform::linux_amd64()]),
        };
        let arm = Demand {
            platforms: BTreeSet::from([Platform::linux_arm64()]),
            ..first.clone()
        };
        first.merge(&arm).unwrap();
        assert_eq!(first.platforms.len(), 2);
        let newer = Demand {
            generation: 2,
            version: "1.1.0".into(),
            ..arm.clone()
        };
        first.merge(&newer).unwrap();
        assert_eq!(first.platforms, BTreeSet::from([Platform::linux_arm64()]));
        assert!(first.merge(&arm).is_err());
        assert!(
            first
                .merge(&Demand {
                    image_uid: "other".into(),
                    ..newer
                })
                .is_err()
        );
    }
}

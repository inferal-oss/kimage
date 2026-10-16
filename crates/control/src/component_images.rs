//! Small release catalogs for ordinary, digest-pinned component image pulls.
use anyhow::{Context, Result, ensure};
use kimage_core::oci::{Platform, valid_digest};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

/// Structural validation alone never authorizes a component. Runtime consumers
/// additionally compare it with the administrator-owned installation policy.
pub fn pinned_component(reference: &str, role: &str) -> bool {
    let Some((repository, digest)) = reference.rsplit_once('@') else {
        return false;
    };
    let Some(repository) = repository.strip_suffix(&format!("/{role}")) else {
        return false;
    };
    valid_repository(repository) && valid_digest(digest)
}

pub async fn installed_policy(
    client: kube::Client,
    namespace: &str,
    installation: &str,
) -> Result<ImageCatalog> {
    use kube::ResourceExt;
    let maps: kube::Api<k8s_openapi::api::core::v1::ConfigMap> =
        kube::Api::namespaced(client, namespace);
    let map = maps
        .get(&format!("{installation}-component-images"))
        .await?;
    ensure!(
        map.labels()
            .get(crate::install::LABEL)
            .is_some_and(|label| label == installation)
            && map.metadata.deletion_timestamp.is_none(),
        "ComponentPolicyOwnershipConflict"
    );
    let bytes = map
        .data
        .as_ref()
        .and_then(|data| data.get("images.json"))
        .context("MissingComponentPolicy")?;
    ImageCatalog::parse(bytes.as_bytes(), env!("CARGO_PKG_VERSION"), false)
}

pub async fn authorize_reference(
    client: kube::Client,
    namespace: &str,
    installation: &str,
    role: &str,
    reference: &str,
) -> Result<()> {
    let policy = installed_policy(client, namespace, installation).await?;
    ensure!(
        reference == policy.reference(role)?,
        "UnapprovedComponentImage"
    );
    Ok(())
}

pub async fn authorize_builder_catalog(
    client: kube::Client,
    namespace: &str,
    installation: &str,
    catalog: &serde_json::Value,
) -> Result<()> {
    let policy = installed_policy(client, namespace, installation).await?;
    ensure!(
        catalog["schema"] == 2
            && catalog["builders"]["default"]["image"] == policy.reference("builder")?
            && catalog["builders"]["default"]["inputImage"] == policy.reference("input")?,
        "UnapprovedNativeBuilderCatalog"
    );
    Ok(())
}

pub const ROLES: [&str; 8] = [
    "admission",
    "builder",
    "exporter",
    "gateway",
    "input",
    "operator",
    "proxy",
    "registry",
];
pub const OFFICIAL_REPOSITORY: &str = "ghcr.io/inferal-oss/kimage";
pub const ASSET: &str = "component-images.json";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Component {
    pub version: String,
    pub digest: String,
    pub platforms: BTreeSet<Platform>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageCatalog {
    pub schema: u32,
    pub release: String,
    pub api_schema_revision: u32,
    pub repository: String,
    pub components: BTreeMap<String, Component>,
}

impl ImageCatalog {
    pub fn parse(bytes: &[u8], release: &str, official: bool) -> Result<Self> {
        ensure!(
            !bytes.is_empty() && bytes.len() <= 256 * 1024,
            "ComponentCatalogTooLarge"
        );
        let catalog: Self = serde_json::from_slice(bytes)?;
        catalog.validate(release, official)?;
        Ok(catalog)
    }

    pub fn validate(&self, release: &str, official: bool) -> Result<()> {
        ensure!(
            self.schema == 1 && self.api_schema_revision == crate::schema::REVISION,
            "ComponentCatalogSchemaMismatch"
        );
        let tag = crate::release_bundle::release_tag(release)?;
        let version = kimage_core::selector::exact(tag.trim_start_matches('v'))?;
        ensure!(
            version.major > 0 || version.minor >= 4,
            "ImagePullDeliveryRequires040"
        );
        ensure!(
            self.release == tag.trim_start_matches('v'),
            "ComponentCatalogReleaseMismatch"
        );
        ensure!(
            valid_repository(&self.repository),
            "InvalidComponentRepository"
        );
        ensure!(
            !official || self.repository == OFFICIAL_REPOSITORY,
            "UnofficialComponentRepository"
        );
        ensure!(
            self.components
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>()
                == ROLES.into_iter().collect(),
            "IncompleteComponentCatalog"
        );
        let native = BTreeSet::from([
            Platform::linux_amd64(),
            Platform {
                os: "linux".into(),
                architecture: "arm64".into(),
            },
        ]);
        for (role, component) in &self.components {
            let expected = match role.as_str() {
                "gateway" | "proxy" => crate::serving::VERSION,
                "registry" => "3.1.2",
                "builder" => "0.33.0",
                _ => self.release.as_str(),
            };
            ensure!(
                component.version == expected
                    && valid_digest(&component.digest)
                    && component.platforms == native,
                "InvalidComponentImage: {role}"
            );
        }
        Ok(())
    }

    pub fn reference(&self, role: &str) -> Result<String> {
        let component = self.components.get(role).context("MissingComponentImage")?;
        Ok(format!("{}/{role}@{}", self.repository, component.digest))
    }

    pub async fn resolve(release: &str, local: Option<&Path>) -> Result<Self> {
        let bytes = if let Some(path) = local {
            use std::io::Read;
            ensure!(
                std::fs::symlink_metadata(path)?.is_file(),
                "InvalidComponentCatalogFile"
            );
            let mut bytes = Vec::new();
            std::fs::File::open(path)?
                .take(256 * 1024 + 1)
                .read_to_end(&mut bytes)?;
            bytes
        } else {
            crate::release_bundle::small_release_asset(release, ASSET).await?
        };
        Self::parse(&bytes, release, local.is_none())
    }
}

fn valid_repository(value: &str) -> bool {
    if value.len() > 200
        || value.contains('@')
        || value.contains("//")
        || value.ends_with('/')
        || value
            .bytes()
            .any(|b| !b.is_ascii_lowercase() && !b.is_ascii_digit() && !b".-_:/".contains(&b))
    {
        return false;
    }
    let Some((authority, path)) = value.split_once('/') else {
        return false;
    };
    let Ok(url) = reqwest::Url::parse(&format!("https://{authority}/")) else {
        return false;
    };
    url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn catalog() -> ImageCatalog {
        ImageCatalog {
            schema: 1,
            release: "0.4.0".into(),
            api_schema_revision: crate::schema::REVISION,
            repository: OFFICIAL_REPOSITORY.into(),
            components: ROLES
                .into_iter()
                .map(|role| {
                    (
                        role.into(),
                        Component {
                            version: match role {
                                "gateway" | "proxy" => crate::serving::VERSION,
                                "registry" => "3.1.2",
                                "builder" => "0.33.0",
                                _ => "0.4.0",
                            }
                            .into(),
                            digest: format!("sha256:{}", "a".repeat(64)),
                            platforms: BTreeSet::from([
                                Platform::linux_amd64(),
                                Platform {
                                    os: "linux".into(),
                                    architecture: "arm64".into(),
                                },
                            ]),
                        },
                    )
                })
                .collect(),
        }
    }
    #[test]
    fn catalog_pins_complete_native_images_and_refuses_drift() {
        let good = catalog();
        good.validate("v0.4.0", true).unwrap();
        assert_eq!(
            good.reference("operator").unwrap(),
            format!("{OFFICIAL_REPOSITORY}/operator@sha256:{}", "a".repeat(64))
        );
        for mutation in 0..5 {
            let mut bad = good.clone();
            match mutation {
                0 => {
                    bad.components.remove("builder");
                }
                1 => {
                    bad.components
                        .get_mut("input")
                        .unwrap()
                        .platforms
                        .remove(&Platform::linux_amd64());
                }
                2 => bad.components.get_mut("proxy").unwrap().digest = "latest".into(),
                3 => bad.components.get_mut("operator").unwrap().version = "0.3.0".into(),
                _ => bad.repository = "ghcr.io/another/project".into(),
            }
            assert!(bad.validate("0.4.0", true).is_err());
        }
        assert!(good.validate("0.4.1", true).is_err());
    }
    #[test]
    fn independently_selected_mirrors_still_require_complete_pinned_catalogs() {
        let mut mirror = catalog();
        mirror.repository = "registry.example.test:5443/mirror/kimage".into();
        mirror.validate("0.4.0", false).unwrap();
        for repository in [
            "https://ghcr.io/a",
            "ghcr.io/a/../b",
            "user:secret@registry.example/a",
            "ghcr.io/a?tag=latest",
            "ghcr.io/a/",
        ] {
            mirror.repository = repository.into();
            assert!(mirror.validate("0.4.0", false).is_err());
        }
    }

    #[tokio::test]
    async fn runtime_policy_rejects_other_images_roles_owners_and_releases() {
        use std::sync::{Arc, RwLock};
        let mut policy = catalog();
        policy.release = env!("CARGO_PKG_VERSION").into();
        for (role, component) in &mut policy.components {
            if !["gateway", "proxy", "registry", "builder"].contains(&role.as_str()) {
                component.version = policy.release.clone();
            }
        }
        let map = serde_json::json!({"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":"install-component-images","namespace":"system","labels":{crate::install::LABEL:"install"}},"data":{"images.json":serde_json::to_string(&policy).unwrap()}});
        let current = Arc::new(RwLock::new(map));
        let state = current.clone();
        let router = axum::Router::new().route(
            "/api/v1/namespaces/system/configmaps/install-component-images",
            axum::routing::get(move || {
                let value = state.read().unwrap().clone();
                async move { axum::Json(value) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let client = kube::Client::try_from(kube::Config::new(
            format!("http://{address}").parse().unwrap(),
        ))
        .unwrap();
        authorize_reference(
            client.clone(),
            "system",
            "install",
            "proxy",
            &policy.reference("proxy").unwrap(),
        )
        .await
        .unwrap();
        assert!(
            authorize_reference(
                client.clone(),
                "system",
                "install",
                "proxy",
                &policy.reference("input").unwrap()
            )
            .await
            .is_err()
        );
        let mut builder = serde_json::json!({"schema":2,"builders":{"default":{"image":policy.reference("builder").unwrap(),"inputImage":policy.reference("input").unwrap()}}});
        authorize_builder_catalog(client.clone(), "system", "install", &builder)
            .await
            .unwrap();
        builder["builders"]["default"]["inputImage"] =
            serde_json::json!(format!("ghcr.io/another/input@sha256:{}", "a".repeat(64)));
        assert!(
            authorize_builder_catalog(client.clone(), "system", "install", &builder)
                .await
                .is_err()
        );
        current.write().unwrap()["metadata"]["labels"][crate::install::LABEL] =
            serde_json::json!("other");
        assert!(
            installed_policy(client.clone(), "system", "install")
                .await
                .is_err()
        );
        current.write().unwrap()["metadata"]["labels"][crate::install::LABEL] =
            serde_json::json!("install");
        policy.release = "99.0.0".into();
        current.write().unwrap()["data"]["images.json"] =
            serde_json::json!(serde_json::to_string(&policy).unwrap());
        assert!(installed_policy(client, "system", "install").await.is_err());
        server.abort();
    }
}

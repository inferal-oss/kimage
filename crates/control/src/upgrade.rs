//! Native API-only serving binary rollout. The explicit filesystem compatibility
//! window is Distribution 3.1.1/3.1.2 and serving protocol 0.1.0/0.2.0/0.3.0.
//! Neither direction changes storage format or the installed API schema.
use crate::{
    bundle::Bundle,
    install::{Installer, LABEL},
    node_install::{ENDPOINT_ROLE, Endpoint},
    operation_lease::OperationLease,
};
use anyhow::{Context, Result, ensure};
use k8s_openapi::api::core::v1::{ConfigMap, Node, Pod};
use kube::{
    Api, ResourceExt,
    api::{DeleteParams, ListParams, Preconditions},
};
use serde_json::json;
use std::{collections::BTreeSet, time::Duration};

pub fn compatible(previous: &Bundle, next: &Bundle) -> Result<()> {
    ensure!(
        previous.inventory.api_schema_revision == next.inventory.api_schema_revision,
        "ServingUpgradeSchemaConflict"
    );
    ensure!(
        previous.inventory.platform == next.inventory.platform,
        "UpgradePlatformConflict"
    );
    ensure!(
        previous
            .inventory
            .components
            .keys()
            .eq(next.inventory.components.keys()),
        "UpgradeComponentSetConflict"
    );
    for (role, component) in &previous.inventory.components {
        let target = &next.inventory.components[role];
        match role.as_str() {
            "registry" => ensure!(
                [component.version.as_str(), target.version.as_str()]
                    .iter()
                    .all(|v| matches!(*v, "3.1.1" | "3.1.2")),
                "RegistryMigrationRequired"
            ),
            "gateway" | "proxy" => ensure!(
                [component.version.as_str(), target.version.as_str()]
                    .iter()
                    .all(|v| matches!(*v, "0.1.0" | "0.2.0" | "0.3.0")),
                "UnsupportedServingVersionTransition"
            ),
            _ => {
                ensure!(
                    component.version == target.version && component.root == target.root,
                    "UseControlPlaneUpgradeForOtherComponents"
                );
            }
        }
    }
    Ok(())
}
fn filesystem_profile(config: &ConfigMap, installation: &str) -> Result<()> {
    ensure!(
        config
            .labels()
            .get(LABEL)
            .is_some_and(|v| v == installation)
            && config.immutable == Some(true)
            && config.binary_data.as_ref().is_none_or(|v| v.is_empty())
            && config.data.as_ref().is_some_and(|data| data.len() == 1
                && data
                    .get("config.yml")
                    .is_some_and(|value| value == crate::install::REGISTRY_CONFIG)),
        "UnsupportedRegistryStorageProfile"
    );
    Ok(())
}
impl Installer {
    fn serving_ref(&self, bundle: &Bundle, role: &str) -> Result<String> {
        let c = bundle
            .inventory
            .components
            .get(role)
            .context("MissingServingComponent")?;
        Ok(format!(
            "127.0.0.1:{}/{}/system-{role}@{}",
            self.options.port, self.options.namespace, c.root.digest
        ))
    }
    async fn warm_serving(&self, bundle: &Bundle, node: &str, role: &str) -> Result<()> {
        let key = kimage_core::oci::digest(format!("{}:{node}:{role}", bundle.digest).as_bytes());
        let name = self.name(&format!("uw-{}", &key[7..23]));
        let binary = if role == "registry" {
            "/registry".to_owned()
        } else {
            format!("/kimage-{role}")
        };
        self.owned(json!({"apiVersion":"v1","kind":"Pod","metadata":self.metadata(&name),"spec":{"nodeName":node,"automountServiceAccountToken":false,"restartPolicy":"Never","containers":[{"name":"warmup","image":self.serving_ref(bundle,role)?,"imagePullPolicy":"IfNotPresent","command":[binary,"--help"],"securityContext":{"allowPrivilegeEscalation":false,"readOnlyRootFilesystem":true,"capabilities":{"drop":["ALL"]}},"resources":{"requests":{"cpu":"10m","memory":"16Mi"},"limits":{"cpu":"500m","memory":"128Mi"}}}]}})).await?;
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.options.namespace);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
        loop {
            let pod = pods.get(&name).await?;
            let phase = pod.status.as_ref().and_then(|s| s.phase.as_deref());
            if phase == Some("Succeeded") {
                return Ok(());
            }
            ensure!(phase != Some("Failed"), "UpgradeWarmupFailed");
            ensure!(
                tokio::time::Instant::now() < deadline,
                "UpgradeWarmupTimeout"
            );
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
    /// Resumable upgrades and rollback use the same protocol, with independently
    /// trusted source and target bundles. Only serving images change in this API.
    pub async fn upgrade_serving(&self, previous: &Bundle, next: &Bundle) -> Result<()> {
        compatible(previous, next)?;
        if self.scope().cluster_wide {
            for role in ["gateway", "proxy"] {
                ensure!(
                    next.inventory.components[role].version == "0.3.0",
                    "ClusterScopeRequiresServingVersion030"
                );
            }
        }
        self.preflight_bundle(next).await?;
        let identity =
            kimage_core::oci::digest(format!("{}:{}", previous.digest, next.digest).as_bytes());
        let lease = OperationLease::acquire(
            self.client.clone(),
            &self.options.namespace,
            &self.options.installation,
            &self.name("serving-upgrade"),
            &identity[7..],
        )
        .await?;
        let result = self
            .upgrade_serving_locked(previous, next, &identity, &lease)
            .await;
        let released = lease.release().await;
        result?;
        released
    }
    async fn upgrade_serving_locked(
        &self,
        previous: &Bundle,
        next: &Bundle,
        identity: &str,
        lease: &OperationLease,
    ) -> Result<()> {
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.options.namespace);
        let maps: Api<ConfigMap> = Api::namespaced(self.client.clone(), &self.options.namespace);
        filesystem_profile(
            &maps.get(&self.name("registry-config")).await?,
            &self.options.installation,
        )?;
        tracing::info!(installation=%self.options.installation,namespace=%self.options.namespace,operation_id=identity,previous_inventory=%previous.digest,target_inventory=%next.digest,registry_version=%next.inventory.components["registry"].version,proxy_version=%next.inventory.components["proxy"].version,"serving binary transition started");
        let registry_name = self.name("registry");
        let registry_checkpoint = self.name(&format!("up-{}-registry", &identity[7..19]));
        let journal = format!("upgrade-{}", &identity[7..19]);
        let mut names = vec![(registry_name.clone(), None), (self.name("proxy"), None)];
        let page = maps
            .list(
                &ListParams::default()
                    .labels(&format!(
                        "{LABEL}={},kimage.dev/role={ENDPOINT_ROLE}",
                        self.options.installation
                    ))
                    .limit(129),
            )
            .await?;
        ensure!(
            page.items.len() <= 128 && page.metadata.continue_.is_none_or(|s| s.is_empty()),
            "TooManyNodeEndpoints"
        );
        let nodes: Api<Node> = Api::all(self.client.clone());
        for map in page.items {
            let endpoint: Endpoint = serde_json::from_str(
                map.data
                    .as_ref()
                    .and_then(|d| d.get("endpoint.json"))
                    .context("MissingNodeEndpoint")?,
            )?;
            endpoint.validate(
                &self.options.installation,
                &self.options.namespace,
                self.options
                    .application_namespace
                    .as_deref()
                    .context("ApplicationNamespaceRequired")?,
                self.options.port,
            )?;
            let Some(node) = nodes.get_opt(&endpoint.node).await? else {
                continue;
            };
            if node.uid().as_deref() != Some(&endpoint.node_uid) {
                continue;
            }
            let name = format!(
                "{}-proxy",
                crate::node_install::scope(&self.options.installation, &endpoint.node_uid)
            );
            ensure!(
                map.name_any()
                    == format!(
                        "{}-endpoint",
                        crate::node_install::scope(&self.options.installation, &endpoint.node_uid)
                    ),
                "NodeEndpointIdentityConflict"
            );
            names.push((name, Some(endpoint)));
        }
        // Validate or recover every exact template before staging any mutation.
        let mut replacements = Vec::new();
        for (name, endpoint) in names {
            let checkpoint = if name == registry_name {
                registry_checkpoint.clone()
            } else {
                self.name(&format!(
                    "up-{}-{}",
                    &identity[7..19],
                    &kimage_core::oci::digest(name.as_bytes())[7..15]
                ))
            };
            let existing = pods.get_opt(&name).await?;
            let mut desired = if let Some(pod) = &existing {
                ensure!(
                    pod.labels().get(LABEL) == Some(&self.options.installation)
                        && pod.metadata.deletion_timestamp.is_none(),
                    "UpgradeServingOwnershipConflict"
                );
                let mut pod = pod.clone();
                let spec = pod.spec.as_mut().context("MissingServingSpec")?;
                ensure!(
                    spec.node_name.as_ref().is_some_and(
                        |n| n == endpoint.as_ref().map_or(&self.options.node, |e| &e.node)
                    ),
                    "UpgradeNodeConflict"
                );
                let expected_roles: BTreeSet<_> = if name == registry_name {
                    BTreeSet::from(["registry", "gateway"])
                } else {
                    BTreeSet::from(["proxy"])
                };
                ensure!(
                    spec.containers
                        .iter()
                        .map(|c| c.name.as_str())
                        .collect::<BTreeSet<_>>()
                        == expected_roles
                        && spec.containers.len() == expected_roles.len(),
                    "UnexpectedServingComponents"
                );
                for c in &mut spec.containers {
                    if c.name == "registry" {
                        ensure!(
                            c.args
                                .as_ref()
                                .is_some_and(|args| args == &["serve", "/config/config.yml"])
                                && c.env_from.as_ref().is_none_or(|v| v.is_empty())
                                && c.env.as_ref().is_none_or(|env| env
                                    .iter()
                                    .all(|v| !v.name.starts_with("REGISTRY_")))
                                && c.volume_mounts.as_ref().is_some_and(|mounts| mounts
                                    .iter()
                                    .any(|m| m.name == "data"
                                        && m.mount_path == "/data"
                                        && m.read_only != Some(true))
                                    && mounts.iter().any(|m| m.name == "config"
                                        && m.mount_path == "/config"
                                        && m.read_only == Some(true))),
                            "UnsupportedRegistryStorageProfile"
                        );
                        ensure!(
                            spec.volumes
                                .as_ref()
                                .is_some_and(|volumes| volumes.iter().any(|v| v.name == "data"
                                    && v.persistent_volume_claim.as_ref().is_some_and(|p| p
                                        .claim_name
                                        == self
                                            .options
                                            .pvc
                                            .clone()
                                            .unwrap_or_else(|| self.name("data"))
                                        && p.read_only != Some(true)))
                                    && volumes.iter().any(|v| v.name == "config"
                                        && v.config_map.as_ref().is_some_and(
                                            |m| m.name == self.name("registry-config")
                                        ))),
                            "UnsupportedRegistryStorageProfile"
                        );
                    }
                    ensure!(
                        c.image.as_deref() == Some(&self.serving_ref(previous, &c.name)?)
                            || c.image.as_deref() == Some(&self.serving_ref(next, &c.name)?),
                        "UpgradeSourceImageConflict"
                    );
                    ensure!(
                        c.image_pull_policy.as_deref() == Some("IfNotPresent")
                            && c.command.as_ref().is_some_and(|cmd| cmd
                                == &[if c.name == "registry" {
                                    "/registry".to_owned()
                                } else {
                                    format!("/kimage-{}", c.name)
                                }]),
                        "UpgradeRequiresOrdinaryServing"
                    );
                    c.image = Some(self.serving_ref(next, &c.name)?);
                    if c.name == "proxy"
                        && !c
                            .args
                            .as_ref()
                            .is_some_and(|a| a.iter().any(|v| v == "--overlap"))
                    {
                        c.args.get_or_insert_default().push("--overlap".into());
                    }
                }
                pod
            } else {
                let map = maps
                    .get(&checkpoint)
                    .await
                    .context("MissingUpgradeCheckpoint: recover from the retained bundle first")?;
                ensure!(
                    map.labels().get(LABEL) == Some(&self.options.installation)
                        && map.data.as_ref().and_then(|d| d.get("bundle")) == Some(&next.digest),
                    "UpgradeCheckpointConflict"
                );
                serde_json::from_str(
                    map.data
                        .as_ref()
                        .and_then(|d| d.get("pod"))
                        .context("MissingUpgradeTemplate")?,
                )?
            };
            let annotations = desired.metadata.annotations.take();
            let labels = desired.metadata.labels.take();
            desired.metadata = serde_json::from_value(self.metadata(&name))?;
            desired.metadata.annotations = annotations;
            desired.metadata.labels = labels;
            desired.status = None;
            ensure!(
                desired.name_any() == name
                    && desired.labels().get(LABEL) == Some(&self.options.installation),
                "InvalidUpgradeTemplate"
            );
            let saved = serde_json::to_string(&desired)?;
            ensure!(saved.len() <= 65536, "UpgradeTemplateTooLarge");
            lease.check().await?;
            self.owned(json!({"apiVersion":"v1","kind":"ConfigMap","metadata":self.metadata(&checkpoint),"immutable":true,"data":{"bundle":next.digest,"pod":saved}})).await?;
            replacements.push((name, existing, desired, endpoint));
        }
        lease.check().await?;
        if replacements.iter().any(|(_, p, _, _)| p.is_none()) {
            // A previous attempt already verified staging and warmed all nodes.
            // Its read-only server remains alive across the missing-writer gap.
            ensure!(
                pods.get_opt(&self.name("readthrough")).await?.is_some(),
                "UpgradeInterruptedWithoutServingPath: recover retained bundle first"
            );
        } else {
            self.seed(next).await?;
        }
        for (_, _, desired, _) in &replacements {
            let spec = desired.spec.as_ref().context("MissingServingSpec")?;
            for c in &spec.containers {
                self.warm_serving(
                    next,
                    spec.node_name.as_deref().context("MissingServingNode")?,
                    &c.name,
                )
                .await?;
            }
        }
        lease.check().await?;
        self.state_named(next, "ReplacementsWarmed", &journal)
            .await?;
        self.prepare_readthrough_from(next, &registry_checkpoint)
            .await?;
        for (name, existing, desired, mut endpoint) in replacements {
            lease.check().await?;
            let is_registry = name == registry_name;
            let overlap_name = if name == self.name("proxy") {
                self.name("proxy-next")
            } else {
                format!("{name}-next")
            };
            if existing.as_ref().is_some_and(|p| {
                p.spec.as_ref().is_some_and(|s| {
                    s.containers.iter().all(|c| {
                        self.serving_ref(next, &c.name)
                            .is_ok_and(|r| c.image.as_ref() == Some(&r))
                    })
                })
            }) {
                self.wait_pod(&name).await?;
                if name == registry_name {
                    self.registry_route("registry").await?;
                } else {
                    if let Some(e) = &mut endpoint {
                        e.proxy_images = BTreeSet::from([self.serving_ref(next, "proxy")?]);
                        self.approve_endpoint(e).await?;
                    }
                    let suffix = overlap_name
                        .strip_prefix(&format!("{}-", self.options.installation))
                        .context("InvalidOverlapName")?;
                    self.remove_transition_pod(suffix).await?;
                }
                continue;
            }
            if !is_registry {
                let mut overlap = desired.clone();
                overlap.metadata.name = Some(overlap_name.clone());
                self.owned(serde_json::to_value(overlap)?).await?;
                self.wait_pod(&overlap_name).await?;
                if let Some(e) = &mut endpoint {
                    e.proxy_images = BTreeSet::from([
                        self.serving_ref(previous, "proxy")?,
                        self.serving_ref(next, "proxy")?,
                    ]);
                    self.approve_endpoint(e).await?;
                }
            } else {
                self.registry_route("readthrough").await?;
            }
            lease.check().await?;
            if let Some(old) = existing {
                // Re-read resourceVersion, preserving the captured UID fence.
                let current = pods.get(&name).await?;
                ensure!(current.uid() == old.uid(), "UpgradePodChanged");
                pods.delete(
                    &name,
                    &DeleteParams {
                        preconditions: Some(Preconditions {
                            uid: current.uid(),
                            resource_version: current.resource_version(),
                        }),
                        ..Default::default()
                    },
                )
                .await?;
                self.wait_absent(&name).await?;
            }
            lease.check().await?;
            self.owned(serde_json::to_value(&desired)?).await?;
            self.wait_pod(&name).await?;
            if is_registry {
                self.registry_route("registry").await?;
            } else {
                if let Some(e) = &mut endpoint {
                    e.proxy_images = BTreeSet::from([self.serving_ref(next, "proxy")?]);
                    self.approve_endpoint(e).await?;
                }
                if let Some(overlap) = pods.get_opt(&overlap_name).await? {
                    ensure!(
                        overlap.labels().get(LABEL) == Some(&self.options.installation),
                        "UpgradeOverlapOwnershipConflict"
                    );
                    pods.delete(
                        &overlap_name,
                        &DeleteParams {
                            preconditions: Some(Preconditions {
                                uid: overlap.uid(),
                                resource_version: overlap.resource_version(),
                            }),
                            ..Default::default()
                        },
                    )
                    .await?;
                    self.wait_absent(&overlap_name).await?;
                }
            }
            self.state_named(next, &format!("Ready:{name}"), &journal)
                .await?;
        }
        lease.check().await?;
        self.registry_route("registry").await?;
        self.remove_transition_pod("readthrough").await?;
        self.remove_transition_pod("proxy-next").await?;
        self.state_named(next, "ServingUpgradeComplete", &journal)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn bundle() -> Bundle {
        let file = json!({"file":"binary","size":1,"digest":format!("sha256:{}","0".repeat(64))});
        let mut components = serde_json::Map::new();
        for role in [
            "registry",
            "gateway",
            "proxy",
            "operator",
            "admission",
            "input",
            "builder",
        ] {
            components.insert(role.into(),json!({"version":if role=="registry" {"3.1.2"} else {"0.1.0"},"executable":file,"archive":file,"root":{"digest":format!("sha256:{}","1".repeat(64)),"size":1,"mediaType":kimage_core::oci::MANIFEST}}));
        }
        Bundle {root:std::path::PathBuf::new(),digest:format!("sha256:{}","2".repeat(64)),inventory:serde_json::from_value(json!({"schema":1,"platform":{"os":"linux","architecture":"amd64"},"launcher":file,"components":components})).unwrap()}
    }
    #[test]
    fn serving_rollout_and_reverse_rollback_keep_storage_and_control_compatibility() {
        let mut old = bundle();
        old.inventory
            .components
            .get_mut("registry")
            .unwrap()
            .version = "3.1.1".into();
        let mut next = bundle();
        for role in ["registry", "gateway", "proxy"] {
            next.inventory.components.get_mut(role).unwrap().root.digest =
                format!("sha256:{}", "3".repeat(64));
        }
        for role in ["gateway", "proxy"] {
            next.inventory.components.get_mut(role).unwrap().version = "0.2.0".into();
            next.inventory
                .components
                .get_mut(role)
                .unwrap()
                .executable
                .digest = format!("sha256:{}", "5".repeat(64));
        }
        assert!(compatible(&old, &next).is_ok());
        assert!(compatible(&next, &old).is_ok());
        next.inventory
            .components
            .get_mut("registry")
            .unwrap()
            .version = "4.0.0".into();
        assert!(compatible(&old, &next).is_err());
    }
    #[test]
    fn serving_command_refuses_implicit_operator_or_schema_downgrade() {
        let old = bundle();
        let mut next = bundle();
        next.inventory
            .components
            .get_mut("operator")
            .unwrap()
            .root
            .digest = format!("sha256:{}", "4".repeat(64));
        assert!(compatible(&old, &next).is_err());
        let mut next = bundle();
        next.inventory.api_schema_revision = Some(5);
        assert!(compatible(&old, &next).is_err());
        for role in ["registry", "gateway", "proxy"] {
            let mut unsupported = bundle();
            unsupported
                .inventory
                .components
                .get_mut(role)
                .unwrap()
                .version = "9.0.0".into();
            assert!(compatible(&old, &unsupported).is_err());
            assert!(compatible(&unsupported, &old).is_err());
        }
    }
    #[test]
    fn binary_compatibility_does_not_authorize_other_storage_profiles() {
        let mut config: ConfigMap = serde_json::from_value(json!({"metadata":{"labels":{LABEL:"fixture"}},"immutable":true,"data":{"config.yml":crate::install::REGISTRY_CONFIG}})).unwrap();
        filesystem_profile(&config, "fixture").unwrap();
        config
            .data
            .as_mut()
            .unwrap()
            .get_mut("config.yml")
            .unwrap()
            .push_str("  storage: s3\n");
        assert!(filesystem_profile(&config, "fixture").is_err());
    }
}

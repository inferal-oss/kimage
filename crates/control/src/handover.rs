//! Read availability during a single-writer transition. The temporary server
//! uses Distribution's supported read-only mode; no second registry writer.
use crate::{
    bundle::Bundle,
    install::{Installer, LABEL},
};
use anyhow::{Context, Result, ensure};
use k8s_openapi::api::core::v1::Pod;
use kube::{
    Api, ResourceExt,
    api::{DeleteParams, Patch, PatchParams, Preconditions},
};
use serde_json::json;
impl Installer {
    /// Delete only the verified bundle's temporary executable delivery objects.
    /// Checkpoints, credentials and retained data remain available for recovery.
    pub async fn cleanup_bootstrap(&self, bundle: &Bundle) -> Result<()> {
        use base64::{Engine, engine::general_purpose::STANDARD};
        use std::collections::BTreeMap;
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.options.namespace);
        for role in ["registry", "proxy"] {
            self.wait_pod(&self.name(role)).await?;
            let pod = pods.get(&self.name(role)).await?;
            ensure!(
                pod.labels().get(LABEL) == Some(&self.options.installation),
                "CleanupServingOwnershipConflict"
            );
            let spec = pod.spec.context("MissingServingSpec")?;
            ensure!(
                spec.containers.iter().all(|c| {
                    bundle
                        .inventory
                        .components
                        .get(&c.name)
                        .is_some_and(|component| {
                            c.image.as_deref()
                                == Some(&format!(
                                    "127.0.0.1:{}/{}/system-{}@{}",
                                    self.options.port,
                                    self.options.namespace,
                                    c.name,
                                    component.root.digest
                                ))
                                && c.image_pull_policy.as_deref() == Some("IfNotPresent")
                        })
                }),
                "CleanupRequiresOrdinaryBundleServing"
            );
        }
        let mut expected = BTreeMap::new();
        expected.insert(self.name("launcher"), json!({"launcher":STANDARD.encode(std::fs::read(bundle.path(&bundle.inventory.launcher)?)?)}));
        for role in ["registry", "gateway", "proxy"] {
            for (index, bytes) in bundle.chunks(role)?.0.iter().enumerate() {
                expected.insert(
                    self.name(&format!("{role}-{index:04}")),
                    json!({format!("{index:04}"):STANDARD.encode(bytes)}),
                );
            }
        }
        let mut token = None;
        let mut count = 0;
        loop {
            let mut params = kube::api::ListParams::default().limit(256);
            params.continue_token = token;
            let page = pods.list(&params).await?;
            count += page.items.len();
            ensure!(count <= 4096, "CleanupPodInventoryTooLarge");
            for pod in page.items {
                if pod
                    .status
                    .as_ref()
                    .and_then(|s| s.phase.as_deref())
                    .is_some_and(|p| matches!(p, "Succeeded" | "Failed"))
                {
                    continue;
                }
                for volume in pod
                    .spec
                    .as_ref()
                    .and_then(|s| s.volumes.as_ref())
                    .into_iter()
                    .flatten()
                {
                    let direct = volume.config_map.as_ref().map(|c| &c.name);
                    let projected = volume
                        .projected
                        .as_ref()
                        .and_then(|v| v.sources.as_ref())
                        .into_iter()
                        .flatten()
                        .filter_map(|s| s.config_map.as_ref().map(|c| &c.name));
                    ensure!(
                        !direct
                            .into_iter()
                            .chain(projected)
                            .any(|name| expected.contains_key(name)),
                        "BootstrapPayloadStillInUse"
                    );
                }
            }
            token = page.metadata.continue_.filter(|s| !s.is_empty());
            if token.is_none() {
                break;
            }
        }
        let api = self.api_at("ConfigMap", Some(&self.options.namespace))?;
        for (name, data) in expected {
            let Some(current) = api.get_opt(&name).await? else {
                continue;
            };
            ensure!(
                current.labels().get(LABEL) == Some(&self.options.installation)
                    && current.data["immutable"] == true
                    && current.data["binaryData"] == data,
                "BootstrapCleanupPayloadConflict"
            );
            api.delete(
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
        }
        Ok(())
    }
    pub(crate) async fn registry_route(&self, role: &str) -> Result<()> {
        let api = self.api_at("Service", Some(&self.options.namespace))?;
        let name = self.name("registry");
        let current = api.get(&name).await?;
        ensure!(
            current.labels().get(LABEL) == Some(&self.options.installation)
                && current.data["spec"]["selector"][LABEL] == self.options.installation
                && matches!(
                    current.data["spec"]["selector"]["kimage.dev/role"].as_str(),
                    Some("registry" | "readthrough")
                ),
            "RegistryRouteOwnershipConflict"
        );
        api.patch(&name, &PatchParams::default(), &Patch::Merge(json!({"metadata":{"resourceVersion":current.resource_version()},"spec":{"selector":{LABEL:self.options.installation,"kimage.dev/role":role}}}))).await?;
        self.wait_service_target(&name, &self.name(role)).await
    }
    pub(crate) async fn wait_service_target(&self, name: &str, expected: &str) -> Result<()> {
        let slices: Api<k8s_openapi::api::discovery::v1::EndpointSlice> =
            Api::namespaced(self.client.clone(), &self.options.namespace);
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.options.namespace);
        let uid = pods
            .get(expected)
            .await?
            .uid()
            .context("MissingEndpointPodUid")?;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let endpoints = slices
                .list(
                    &kube::api::ListParams::default()
                        .labels(&format!("kubernetes.io/service-name={name}")),
                )
                .await?;
            let ready: Vec<_> = endpoints
                .items
                .iter()
                .flat_map(|s| s.endpoints.iter().flatten())
                .filter(|e| e.conditions.as_ref().is_some_and(|c| c.ready == Some(true)))
                .collect();
            if !ready.is_empty()
                && ready.iter().all(|e| {
                    e.target_ref.as_ref().is_some_and(|r| {
                        r.name.as_deref() == Some(expected) && r.uid.as_deref() == Some(&uid)
                    })
                })
            {
                break;
            }
            ensure!(
                tokio::time::Instant::now() < deadline,
                "ServiceEndpointReadinessTimeout"
            );
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        Ok(())
    }
    pub(crate) async fn prepare_readthrough(&self, bundle: &Bundle) -> Result<()> {
        self.prepare_readthrough_from(bundle, &self.name("replacement-registry"))
            .await
    }
    pub(crate) async fn prepare_readthrough_from(
        &self,
        bundle: &Bundle,
        checkpoint_name: &str,
    ) -> Result<()> {
        let config = "version: 0.1\nlog:\n  level: warn\nstorage:\n  filesystem:\n    rootdirectory: /data\n  maintenance:\n    uploadpurging:\n      enabled: false\n    readonly:\n      enabled: true\n  delete:\n    enabled: false\nhttp:\n  addr: 127.0.0.1:5000\n  host: http://127.0.0.1:5000\n";
        self.owned(json!({"apiVersion":"v1","kind":"ConfigMap","metadata":self.metadata(&self.name("readthrough-config")),"immutable":true,"data":{"config.yml":config}})).await?;
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.options.namespace);
        if let Some(reader) = pods.get_opt(&self.name("readthrough")).await? {
            ensure!(
                reader.labels().get(LABEL) == Some(&self.options.installation),
                "TransitionOwnershipConflict"
            );
            let spec = reader.spec.as_ref().context("MissingReadthroughSpec")?;
            ensure!(
                spec.volumes
                    .as_ref()
                    .is_some_and(|vs| vs.iter().any(|v| v.name == "data"
                        && v.persistent_volume_claim
                            .as_ref()
                            .is_some_and(|p| p.read_only == Some(true)))),
                "ReadthroughVolumeNotReadOnly"
            );
            ensure!(
                spec.containers.iter().all(|c| bundle
                    .inventory
                    .components
                    .get(&c.name)
                    .is_some_and(|component| c.image.as_deref()
                        == Some(&format!(
                            "127.0.0.1:{}/{}/system-{}@{}",
                            self.options.port,
                            self.options.namespace,
                            c.name,
                            component.root.digest
                        )))),
                "ReadthroughBundleConflict"
            );
            self.wait_pod(&self.name("readthrough")).await?;
            return Ok(());
        }
        let existing = if let Some(existing) = pods.get_opt(&self.name("registry")).await? {
            existing
        } else {
            let checkpoint = self
                .api_at("ConfigMap", Some(&self.options.namespace))?
                .get(checkpoint_name)
                .await?;
            ensure!(
                checkpoint.labels().get(LABEL) == Some(&self.options.installation)
                    && checkpoint.data["data"]["bundle"] == bundle.digest,
                "HandoverCheckpointConflict"
            );
            serde_json::from_str(
                checkpoint.data["data"]["pod"]
                    .as_str()
                    .context("InvalidHandoverCheckpoint")?,
            )?
        };
        ensure!(
            existing.labels().get(LABEL) == Some(&self.options.installation),
            "ServingOwnershipConflict"
        );
        let mut reader = existing;
        reader.metadata = serde_json::from_value(self.metadata(&self.name("readthrough")))?;
        reader
            .metadata
            .labels
            .get_or_insert_default()
            .insert("kimage.dev/role".into(), "readthrough".into());
        reader.status = None;
        let spec = reader.spec.as_mut().context("MissingServingSpec")?;
        for container in &mut spec.containers {
            let role = &container.name;
            ensure!(
                matches!(role.as_str(), "registry" | "gateway"),
                "UnexpectedServingComponent"
            );
            let component = bundle
                .inventory
                .components
                .get(role)
                .context("MissingServingComponent")?;
            container.image = Some(format!(
                "127.0.0.1:{}/{}/system-{role}@{}",
                self.options.port, self.options.namespace, component.root.digest
            ));
            container.image_pull_policy = Some("IfNotPresent".into());
            if container
                .command
                .as_ref()
                .is_some_and(|c| c == &["/bootstrap/launcher"])
            {
                let args = container
                    .args
                    .as_ref()
                    .context("MissingLauncherArguments")?;
                ensure!(args.len() >= 4, "InvalidLauncherArguments");
                container.args = Some(args[4..].to_vec());
            }
            container.command = Some(vec![if role == "registry" {
                "/registry".into()
            } else {
                "/kimage-gateway".into()
            }]);
            if let Some(mounts) = &mut container.volume_mounts {
                mounts.retain(|m| {
                    m.name != "launcher" && m.name != "work" && !m.name.ends_with("-payload")
                });
                for mount in mounts {
                    if mount.name == "data" {
                        mount.read_only = Some(true);
                    }
                }
            }
        }
        if let Some(volumes) = &mut spec.volumes {
            volumes.retain(|v| {
                v.name != "launcher" && v.name != "work" && !v.name.ends_with("-payload")
            });
            for volume in volumes {
                if volume.name == "data" {
                    volume
                        .persistent_volume_claim
                        .as_mut()
                        .context("MissingRegistryVolume")?
                        .read_only = Some(true);
                }
                if volume.name == "config" {
                    volume
                        .config_map
                        .as_mut()
                        .context("MissingRegistryConfiguration")?
                        .name = self.name("readthrough-config");
                }
            }
        }
        self.owned(serde_json::to_value(reader)?).await?;
        self.wait_pod(&self.name("readthrough")).await?;
        // Independently establish serving TLS before changing the Service route.
        let tunnel = crate::tunnel::Tunnel::open(
            self.client.clone(),
            &self.options.namespace,
            &self.name("readthrough"),
            5443,
        )
        .await?;
        let component = &bundle.inventory.components["registry"];
        let registry = crate::registry::Registry::new(
            self.origin().parse()?,
            format!("{}/system-registry", self.options.namespace),
            &std::fs::read(self.options.credentials.join("ca.crt"))?,
            Some(tunnel.address),
            "reader".into(),
            std::fs::read_to_string(self.options.credentials.join("reader-password"))?
                .trim_end()
                .into(),
        )?;
        registry
            .fetch_verified(
                &component.root,
                &bundle.inventory.platform,
                &tokio_util::sync::CancellationToken::new(),
            )
            .await?;
        Ok(())
    }
    pub(crate) async fn prepare_proxy_overlap(&self, ordinary: &Pod) -> Result<()> {
        let mut next = ordinary.clone();
        next.metadata = serde_json::from_value(self.metadata(&self.name("proxy-next")))?;
        next.metadata
            .labels
            .get_or_insert_default()
            .insert("kimage.dev/role".into(), "proxy-next".into());
        next.metadata.annotations = ordinary.metadata.annotations.clone();
        next.status = None;
        let spec = next.spec.as_mut().context("MissingProxySpec")?;
        ensure!(
            spec.host_network == Some(true) && spec.containers.len() == 1,
            "UnexpectedProxyDefinition"
        );
        let args = spec.containers[0].args.get_or_insert_default();
        if !args.iter().any(|a| a == "--overlap") {
            args.push("--overlap".into());
        }
        self.owned(serde_json::to_value(next)?).await?;
        self.wait_pod(&self.name("proxy-next")).await
    }
    pub(crate) async fn remove_transition_pod(&self, role: &str) -> Result<()> {
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.options.namespace);
        let name = self.name(role);
        if let Some(pod) = pods.get_opt(&name).await? {
            ensure!(
                pod.labels().get(LABEL) == Some(&self.options.installation),
                "TransitionOwnershipConflict"
            );
            pods.delete(
                &name,
                &DeleteParams {
                    preconditions: Some(Preconditions {
                        uid: pod.uid(),
                        resource_version: pod.resource_version(),
                    }),
                    ..Default::default()
                },
            )
            .await?;
            self.wait_absent(&name).await?;
        }
        Ok(())
    }
}

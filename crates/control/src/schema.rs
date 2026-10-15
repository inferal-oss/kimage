//! Built-in-only schema startup; never deletes CRDs or forces field ownership.
use crate::api::{Image, ImageRelease};
use anyhow::{Context, Result, bail, ensure};
use k8s_openapi::{
    api::{
        coordination::v1::{Lease, LeaseSpec},
        core::v1::ConfigMap,
    },
    apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition as Crd,
    apimachinery::pkg::apis::meta::v1::{MicroTime, ObjectMeta},
    jiff::Timestamp,
};
use kube::{
    Api, Client, CustomResourceExt, ResourceExt,
    api::{Patch, PatchParams, PostParams},
};
use serde_json::{Value, json};
const MANAGER: &str = "kimage-schema-manager";
pub const REVISION: u32 = 5;
fn nonnullable(v: &mut Value) {
    match v {
        Value::Object(m) => {
            m.remove("nullable");
            if m.get("format")
                .and_then(Value::as_str)
                .is_some_and(|f| matches!(f, "uint64" | "uint32"))
            {
                m.remove("format");
            }
            for v in m.values_mut() {
                nonnullable(v);
            }
        }
        Value::Array(a) => {
            for v in a {
                nonnullable(v);
            }
        }
        _ => {}
    }
}
pub fn definitions() -> Result<Vec<Crd>> {
    let mut result = Vec::new();
    for (crd, release) in [(Image::crd(), false), (ImageRelease::crd(), true)] {
        let mut v = serde_json::to_value(crd)?;
        let spec = &mut v["spec"]["versions"][0]["schema"]["openAPIV3Schema"]["properties"]["spec"];
        nonnullable(spec);
        let version = &mut spec["properties"]["version"];
        version["maxLength"] = json!(128);
        version["pattern"] = json!(
            r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-((0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)(\.(0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*))?$"
        );
        if release {
            spec["x-kubernetes-validations"] = json!([
                {"rule":"self == oldSelf","message":"published identity and content are immutable"},
                {"rule":"self.platform.architecture != 'auto' || self.root.mediaType == 'application/vnd.oci.image.index.v1+json'","message":"aggregate releases require an OCI index"}
            ]);
            for field in [
                "imageName",
                "imageUid",
                "snapshot",
                "requestDigest",
                "publisherVersion",
            ] {
                spec["properties"][field]["maxLength"] = json!(253);
            }
            spec["properties"]["snapshot"]["pattern"] = json!("^sha256:[0-9a-f]{64}$");
            let provenance = &mut spec["properties"]["provenance"]["properties"];
            for field in ["format", "adapterVersion"] {
                provenance[field]["maxLength"] = json!(128);
            }
            provenance["inputs"]["maxItems"] = json!(64);
            for field in ["kind", "name", "uid", "resourceVersion"] {
                provenance["inputs"]["items"]["properties"][field]["maxLength"] = json!(253);
            }
            let execution = &mut provenance["execution"]["properties"];
            for field in [
                "builder",
                "engine",
                "engineVersion",
                "adapterVersion",
                "workerUid",
                "engineWorkerId",
                "engineOperationId",
                "attemptId",
            ] {
                execution[field]["maxLength"] = json!(253);
            }
            for field in ["cachedExecutionSteps", "executedSteps"] {
                execution[field]["minimum"] = json!(0);
                execution[field]["maximum"] = json!(8192);
            }
            for field in ["os", "architecture"] {
                execution["platform"]["properties"][field]["maxLength"] = json!(32);
            }
        } else {
            crate::export_schema::apply(spec);
            // Externally tagged Rust enum is represented as one structural object.
            let source = &mut spec["properties"]["source"];
            *source = crate::build_schema::source(source)?;
            let a = &mut source["properties"]["ociArchive"]["properties"];
            a["producerHandoff"]["enum"] = json!([true]);
            a["path"]["maxLength"] = json!(4096);
            a["path"]["pattern"] = json!(
                r"^([^./\\\x00][^/\\\x00]*|\.[^./\\\x00][^/\\\x00]*|\.\.[^/\\\x00]+)(/([^./\\\x00][^/\\\x00]*|\.[^./\\\x00][^/\\\x00]*|\.\.[^/\\\x00]+))*$"
            );
            spec["properties"]["channels"]["maxProperties"] = json!(32);
            spec["properties"]["channels"]["additionalProperties"] =
                spec["properties"]["version"].clone();
            spec["properties"]["channels"]["x-kubernetes-validations"] = json!([{"rule":"self.all(k, k.matches('^[a-z][a-z0-9-]{0,61}[a-z0-9]$') || k.matches('^[a-z]$'))","message":"channels must be non-numeric DNS labels"}]);
        }
        let platform = &mut spec["properties"]["platform"]["properties"];
        platform["os"]["enum"] = json!(["linux"]);
        platform["architecture"]["enum"] = json!(["amd64", "arm64", "auto"]);
        if !release {
            let platforms = &mut spec["properties"]["platforms"];
            platforms["maxItems"] = json!(2);
            platforms["x-kubernetes-list-type"] = json!("set");
            platforms["items"]["x-kubernetes-map-type"] = json!("atomic");
            platforms["items"]["properties"]["os"]["enum"] = json!(["linux"]);
            platforms["items"]["properties"]["architecture"]["enum"] = json!(["amd64", "arm64"]);
            spec["x-kubernetes-validations"] = json!([{"rule":"!has(self.platform) || self.platform.architecture == 'auto' || !has(self.platforms) || size(self.platforms) == 0","message":"use platform or platforms, not both"}]);
        }
        let root = if release {
            &mut spec["properties"]["root"]
        } else {
            &mut spec["properties"]["source"]["properties"]["ociArchive"]["properties"]["root"]
        };
        root["properties"]["digest"]["pattern"] = json!("^sha256:[0-9a-f]{64}$");
        root["properties"]["digest"]["maxLength"] = json!(71);
        root["properties"]["mediaType"]["maxLength"] = json!(128);
        root["properties"]["annotations"]["maxProperties"] = json!(32);
        root["properties"]["annotations"]["additionalProperties"]["maxLength"] = json!(1024);
        for key in ["os", "architecture"] {
            root["properties"]["platform"]["properties"][key]["maxLength"] = json!(32);
        }
        root["properties"]["size"]["minimum"] = json!(1);
        root["properties"]["size"]["maximum"] = json!(2097152);
        root["properties"]["mediaType"]["enum"] =
            json!([kimage_core::oci::MANIFEST, kimage_core::oci::INDEX]);
        if !release {
            crate::export_schema::status(
                &mut v["spec"]["versions"][0]["schema"]["openAPIV3Schema"]["properties"]["status"],
            );
        }
        v["metadata"]["annotations"] = json!({"kimage.dev/schema-manager":MANAGER,"kimage.dev/schema-revision":REVISION.to_string(),"kimage.dev/schema-digest":kimage_core::oci::digest(&serde_json::to_vec(&v["spec"])?)});
        result.push(serde_json::from_value(v)?);
    }
    let mut demand = serde_json::to_value(crate::configuration::ImageDemand::crd())?;
    nonnullable(&mut demand["spec"]["versions"][0]["schema"]["openAPIV3Schema"]);
    let spec =
        &mut demand["spec"]["versions"][0]["schema"]["openAPIV3Schema"]["properties"]["spec"];
    for field in ["namespace", "imageName", "imageUid", "version"] {
        spec["properties"][field]["maxLength"] = json!(253);
    }
    spec["properties"]["platforms"]["minItems"] = json!(1);
    spec["properties"]["platforms"]["maxItems"] = json!(2);
    spec["properties"]["platforms"]["items"]["properties"]["os"]["enum"] = json!(["linux"]);
    spec["properties"]["platforms"]["items"]["properties"]["architecture"]["enum"] =
        json!(["amd64", "arm64"]);
    demand["metadata"]["annotations"] = json!({"kimage.dev/schema-manager":MANAGER,"kimage.dev/schema-revision":REVISION.to_string(),"kimage.dev/schema-digest":kimage_core::oci::digest(&serde_json::to_vec(&demand["spec"])?)});
    result.push(serde_json::from_value(demand)?);
    Ok(result)
}
pub fn compatibility(existing: &Crd, desired: &Crd) -> Result<()> {
    let revision = |crd: &Crd| {
        crd.metadata
            .annotations
            .as_ref()
            .and_then(|a| a.get("kimage.dev/schema-revision"))
            .cloned()
    };
    if matches!(revision(existing).as_deref(), Some("2" | "3" | "4"))
        && revision(desired).as_deref() == Some("5")
    {
        // Only this shipped historical definition is eligible, never an arbitrary
        // old-looking schema. Existing objects remain in the same storage version.
        let previous: Vec<Crd> =
            serde_json::from_str(if revision(existing).as_deref() == Some("2") {
                include_str!("../schemas/revision-2.json")
            } else if revision(existing).as_deref() == Some("3") {
                include_str!("../schemas/revision-3.json")
            } else {
                include_str!("../schemas/revision-4.json")
            })?;
        let previous = previous
            .iter()
            .find(|c| c.name_any() == existing.name_any())
            .context("UnsupportedSchemaTransition")?;
        return compatibility_exact(existing, previous);
    }
    compatibility_exact(existing, desired)
}
fn compatibility_exact(existing: &Crd, desired: &Crd) -> Result<()> {
    let a = existing
        .metadata
        .annotations
        .as_ref()
        .context("SchemaOwnershipConflict")?;
    ensure!(
        a.get("kimage.dev/schema-manager")
            .is_some_and(|v| v == MANAGER),
        "SchemaOwnershipConflict"
    );
    ensure!(
        a.get("kimage.dev/schema-revision")
            == desired
                .metadata
                .annotations
                .as_ref()
                .and_then(|a| a.get("kimage.dev/schema-revision")),
        "UnsupportedSchemaRevision: preserve existing data; use an implemented migration"
    );
    ensure!(
        existing
            .metadata
            .owner_references
            .as_ref()
            .is_none_or(Vec::is_empty),
        "SchemaDeletionCouplingConflict"
    );
    let mut actual = existing.spec.clone();
    if desired.spec.names.list_kind.is_none()
        && actual.names.list_kind.as_deref() == Some(&format!("{}List", actual.names.kind))
    {
        actual.names.list_kind = None;
    }
    if actual
        .conversion
        .as_ref()
        .is_some_and(|c| c.strategy == "None" && c.webhook.is_none())
    {
        actual.conversion = None;
    }
    ensure!(actual == desired.spec, "SchemaDefinitionConflict");
    ensure!(
        existing
            .metadata
            .managed_fields
            .as_ref()
            .is_none_or(
                |entries| entries.iter().all(|entry| entry.subresource.as_deref()
                    == Some("status")
                    || entry.manager.as_deref() == Some(MANAGER)
                    || !entry
                        .fields_v1
                        .as_ref()
                        .is_some_and(|f| f.0.get("f:spec").is_some()))
            ),
        "SchemaFieldOwnershipConflict"
    );
    ensure!(
        a.get("kimage.dev/schema-digest")
            == desired
                .metadata
                .annotations
                .as_ref()
                .unwrap()
                .get("kimage.dev/schema-digest"),
        "SchemaDigestConflict"
    );
    Ok(())
}
pub struct SchemaManager {
    client: Client,
    namespace: String,
    identity: String,
}
impl SchemaManager {
    pub fn new(client: Client, namespace: String, identity: String) -> Self {
        Self {
            client,
            namespace,
            identity,
        }
    }
    async fn acquire(&self) -> Result<()> {
        let api: Api<Lease> = Api::namespaced(self.client.clone(), &self.namespace);
        let now = Timestamp::now();
        let name = "kimage-schema-manager";
        let mut lease = api.get_opt(name).await?;
        if let Some(existing) = &lease {
            let spec = existing.spec.as_ref().context("InvalidSchemaLease")?;
            let expiry = spec
                .renew_time
                .as_ref()
                .map(|t| t.0.as_second() + i64::from(spec.lease_duration_seconds.unwrap_or(30)));
            if spec.holder_identity.as_deref() != Some(&self.identity)
                && expiry.is_none_or(|t| t > now.as_second())
            {
                bail!("SchemaLeaseHeld");
            }
        }
        let new = lease.is_none();
        let value = lease.get_or_insert_with(|| Lease {
            metadata: ObjectMeta {
                name: Some(name.into()),
                ..Default::default()
            },
            spec: None,
        });
        value.spec = Some(LeaseSpec {
            holder_identity: Some(self.identity.clone()),
            renew_time: Some(MicroTime(now)),
            lease_duration_seconds: Some(30),
            ..Default::default()
        });
        if new {
            api.create(&PostParams::default(), value).await?;
        } else {
            api.replace(name, &PostParams::default(), value).await?;
        }
        Ok(())
    }
    pub async fn reconcile(&self) -> Result<()> {
        let desired = definitions()?;
        let api: Api<Crd> = Api::all(self.client.clone());
        for d in &desired {
            if let Some(current) = api.get_opt(&d.name_any()).await? {
                compatibility(&current, d)?;
            }
        }
        self.acquire().await?;
        for d in desired {
            self.acquire().await?;
            let name = d.name_any();
            if let Some(current) = api.get_opt(&name).await? {
                compatibility(&current, &d)?;
                if current.annotations().get("kimage.dev/schema-revision")
                    != d.annotations().get("kimage.dev/schema-revision")
                {
                    let patch = json!({"metadata":{"uid":current.uid(),"resourceVersion":current.resource_version(),"annotations":d.metadata.annotations},"spec":d.spec});
                    let mut params = PatchParams {
                        field_manager: Some(MANAGER.into()),
                        dry_run: true,
                        ..Default::default()
                    };
                    api.patch(&name, &params, &Patch::Merge(&patch)).await?;
                    self.acquire().await?;
                    params.dry_run = false;
                    api.patch(&name, &params, &Patch::Merge(patch)).await?;
                }
            } else {
                api.create(
                    &PostParams {
                        field_manager: Some(MANAGER.into()),
                        ..Default::default()
                    },
                    &d,
                )
                .await?;
            }
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
            loop {
                self.acquire().await?;
                let current = api.get(&name).await?;
                let conditions = current
                    .status
                    .and_then(|s| s.conditions)
                    .unwrap_or_default();
                if ["NamesAccepted", "Established"].iter().all(|kind| {
                    conditions
                        .iter()
                        .any(|c| c.type_ == *kind && c.status == "True")
                }) {
                    break;
                }
                ensure!(
                    tokio::time::Instant::now() < deadline,
                    "SchemaEstablishmentTimeout"
                );
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        }
        let discovery = kube::Discovery::new(self.client.clone())
            .filter(&["kimage.dev"])
            .run()
            .await?;
        let group = discovery
            .get("kimage.dev")
            .context("SchemaDiscoveryUnavailable")?;
        for kind in ["Image", "ImageRelease"] {
            ensure!(
                group
                    .recommended_resources()
                    .iter()
                    .any(|(r, _)| r.kind == kind),
                "SchemaDiscoveryIncomplete"
            );
        }
        self.diagnostic(true, "SchemasEstablished").await
    }
    pub async fn diagnostic(&self, ready: bool, reason: &str) -> Result<()> {
        let api: Api<ConfigMap> = Api::namespaced(self.client.clone(), &self.namespace);
        if let Some(existing) = api.get_opt("kimage-schema-status").await? {
            let legacy = ready
                && existing
                    .data
                    .as_ref()
                    .and_then(|d| d.get("revision"))
                    .is_some_and(|r| matches!(r.as_str(), "2" | "3"));
            if legacy {
                let schemas: Api<Crd> = Api::all(self.client.clone());
                for desired in definitions()? {
                    let current = schemas.get(&desired.name_any()).await?;
                    compatibility_exact(&current, &desired)?;
                }
            }
            ensure!(
                legacy
                    || existing
                        .data
                        .as_ref()
                        .is_some_and(|d| d.get("revision") == Some(&REVISION.to_string())),
                "SchemaDiagnosticOwnershipConflict"
            );
        }
        let v = json!({"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":"kimage-schema-status"},"data":{"ready":ready.to_string(),"reason":reason,"revision":REVISION.to_string()}});
        api.patch(
            "kimage-schema-status",
            &PatchParams::apply(MANAGER),
            &Patch::Apply(v),
        )
        .await?;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn additive_transition_accepts_only_shipped_historical_schemas_and_never_downgrades() {
        let old: Vec<Crd> = [
            include_str!("../schemas/revision-2.json"),
            include_str!("../schemas/revision-3.json"),
            include_str!("../schemas/revision-4.json"),
        ]
        .iter()
        .flat_map(|text| serde_json::from_str::<Vec<Crd>>(text).unwrap())
        .collect();
        for previous in &old {
            let desired = definitions()
                .unwrap()
                .into_iter()
                .find(|c| c.name_any() == previous.name_any())
                .unwrap();
            compatibility(previous, &desired).unwrap();
            assert!(compatibility(&desired, previous).is_err());
        }
        for desired in definitions().unwrap() {
            let Some(previous) = old.iter().find(|c| c.name_any() == desired.name_any()) else {
                continue;
            };
            let before = serde_json::to_value(previous).unwrap();
            compatibility(previous, &desired).unwrap();
            assert_eq!(serde_json::to_value(previous).unwrap(), before);
            assert!(compatibility(&desired, previous).is_err());
            let mut altered = previous.clone();
            altered.spec.versions[0]
                .schema
                .as_mut()
                .unwrap()
                .open_api_v3_schema
                .as_mut()
                .unwrap()
                .description = Some("unowned drift".into());
            assert!(compatibility(&altered, &desired).is_err());
        }
    }
    #[test]
    fn refuses_foreign_and_old_schema_without_mutation() {
        for mut crd in definitions().unwrap() {
            let before = serde_json::to_value(&crd).unwrap();
            assert!(compatibility(&crd, &crd).is_ok());
            let desired = crd.clone();
            crd.metadata.managed_fields = Some(vec![
                k8s_openapi::apimachinery::pkg::apis::meta::v1::ManagedFieldsEntry {
                    manager: Some("foreign".into()),
                    fields_v1: Some(k8s_openapi::apimachinery::pkg::apis::meta::v1::FieldsV1(
                        json!({"f:spec":{}}),
                    )),
                    ..Default::default()
                },
            ]);
            assert!(compatibility(&crd, &desired).is_err());
            crd.metadata.managed_fields = None;
            crd.metadata
                .annotations
                .as_mut()
                .unwrap()
                .insert("kimage.dev/schema-revision".into(), "1".into());
            assert!(compatibility(&crd, &serde_json::from_value(before).unwrap()).is_err());
        }
    }
}

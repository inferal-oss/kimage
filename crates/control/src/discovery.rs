//! Read-only discovery in the current Kubernetes context. Never select a mutation
//! target by list order, and never interpret an unreadable default as absent.
use anyhow::{Result, bail, ensure};
use k8s_openapi::api::core::v1::ConfigMap;
use kube::{Api, Client, ResourceExt, api::ListParams};
use serde::Serialize;
use std::collections::BTreeSet;

pub const DEFAULT_NAMESPACE: &str = "kimage-system";
pub const DEFAULT_INSTALLATION: &str = "kimage";

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Target {
    pub namespace: String,
    pub installation: String,
}
impl Target {
    pub fn flags(&self) -> String {
        format!(
            "--namespace {} --installation {}",
            self.namespace, self.installation
        )
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Installation {
    #[serde(flatten)]
    pub target: Target,
    pub release: Option<String>,
    pub manager: Option<String>,
    pub scope: Option<String>,
    pub application_namespace: Option<String>,
    /// A damaged or unsupported record remains visible; discovery never repairs it.
    pub valid_record: bool,
}

pub fn not_found(target: &Target) -> anyhow::Error {
    anyhow::anyhow!(
        "InstallationNotFound: no kimage installation named '{}' in namespace '{}'. Run 'kimage installations' to find installations in the current Kubernetes context, or use --namespace and --installation to select one.",
        target.installation,
        target.namespace
    )
}
fn read_error(error: kube::Error, action: &str) -> anyhow::Error {
    match error {
        kube::Error::Api(response) if response.code == 403 => anyhow::anyhow!(
            "InstallationAccessDenied: cannot {action}. Check ConfigMap get/list permissions, or select an accessible installation with --namespace and --installation."
        ),
        kube::Error::Api(response) => anyhow::anyhow!(
            "InstallationLookupFailed: cannot {action} (Kubernetes HTTP {}). Check the current context and API access.",
            response.code
        ),
        error => anyhow::anyhow!("InstallationLookupFailed: cannot {action}: {error}"),
    }
}

pub async fn list(
    client: Client,
    namespace: Option<&str>,
    installation: Option<&str>,
) -> Result<Vec<Installation>> {
    validate(namespace, installation)?;
    let api: Api<ConfigMap> = match namespace {
        Some(namespace) => Api::namespaced(client, namespace),
        None => Api::all(client),
    };
    let label = installation.map_or_else(
        || crate::install::LABEL.to_owned(),
        |id| format!("{}={id}", crate::install::LABEL),
    );
    let mut continuation = String::new();
    let mut seen = BTreeSet::new();
    let mut found = Vec::new();
    loop {
        ensure!(
            seen.len() < 64 && seen.insert(continuation.clone()),
            "InstallationDiscoveryLimit: narrow discovery with --namespace"
        );
        let mut params = ListParams::default().labels(&label).limit(256);
        if !continuation.is_empty() {
            params = params.continue_token(&continuation);
        }
        let page = api.list(&params).await.map_err(|e| read_error(e, "list installation ConfigMaps; cluster discovery requires list access across namespaces"))?;
        for map in page.items {
            let Some(id) = map.labels().get(crate::install::LABEL).cloned() else {
                continue;
            };
            let Some(ns) = map.namespace() else {
                continue;
            };
            if map.metadata.deletion_timestamp.is_some()
                || map.name_any() != format!("{id}-installation")
            {
                continue;
            }
            validate(Some(&ns), Some(&id))?;
            let state = crate::lifecycle::decode_state(&map, &ns, &id);
            let (release, scope, application_namespace) = match &state {
                Ok((options, release)) => (
                    Some(release.clone()),
                    Some(
                        if options.scope == crate::install::InstallScope::Cluster {
                            "cluster"
                        } else {
                            "namespace"
                        }
                        .into(),
                    ),
                    options.application_namespace.clone(),
                ),
                Err(_) => (None, None, None),
            };
            found.push(Installation {
                target: Target {
                    namespace: ns,
                    installation: id,
                },
                release,
                manager: map.annotations().get("kimage.dev/manager").cloned(),
                scope,
                application_namespace,
                valid_record: state.is_ok(),
            });
        }
        continuation = page.metadata.continue_.unwrap_or_default();
        if continuation.is_empty() {
            break;
        }
    }
    found.sort_by(|a, b| {
        (&a.target.namespace, &a.target.installation)
            .cmp(&(&b.target.namespace, &b.target.installation))
    });
    Ok(found)
}
fn validate(namespace: Option<&str>, installation: Option<&str>) -> Result<()> {
    ensure!(
        namespace.is_none_or(kimage_core::selector::dns_label)
            && installation.is_none_or(|id| id.len() <= 32 && kimage_core::selector::dns_label(id)),
        "InvalidInstallationIdentity: use a Kubernetes namespace and an installation name of at most 32 characters"
    );
    Ok(())
}

/// Prefer the conventional target. Explicit selectors constrain fallback discovery;
/// two explicit selectors or --no-discovery disable fallback altogether.
pub async fn select(
    client: Client,
    namespace: Option<&str>,
    installation: Option<&str>,
    no_discovery: bool,
) -> Result<Vec<Target>> {
    validate(namespace, installation)?;
    let default = Target {
        namespace: namespace.unwrap_or(DEFAULT_NAMESPACE).into(),
        installation: installation.unwrap_or(DEFAULT_INSTALLATION).into(),
    };
    let api: Api<ConfigMap> = Api::namespaced(client.clone(), &default.namespace);
    if api
        .get_opt(&format!("{}-installation", default.installation))
        .await
        .map_err(|e| {
            read_error(
                e,
                &format!(
                    "read installation '{}' in namespace '{}'",
                    default.installation, default.namespace
                ),
            )
        })?
        .is_some()
    {
        return Ok(vec![default]);
    }
    if no_discovery || namespace.is_some() && installation.is_some() {
        return Err(not_found(&default));
    }
    let found = list(client, namespace, installation).await?;
    ensure!(
        !found.is_empty(),
        "NoInstallationsFound: no kimage installations found matching this selection in the current Kubernetes context. Check 'kubectl config current-context', run 'kimage installations', or create an installation with 'kimage install'."
    );
    Ok(found.into_iter().map(|i| i.target).collect())
}
pub fn single(targets: Vec<Target>) -> Result<Target> {
    if targets.len() != 1 {
        bail!(
            "MultipleInstallationsFound: select one installation explicitly before changing it:\n{}",
            targets
                .iter()
                .map(|t| format!("  {}", t.flags()))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
    Ok(targets.into_iter().next().unwrap())
}

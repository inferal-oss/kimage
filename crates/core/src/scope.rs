//! Installation scope is administrator configured, never selected by Pod labels.
use crate::selector::dns_label;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NamespaceScope {
    pub system_namespace: String,
    pub application_namespace: Option<String>,
    pub cluster_wide: bool,
    #[serde(default)]
    pub excluded_namespaces: BTreeSet<String>,
}
impl NamespaceScope {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            dns_label(&self.system_namespace)
                && self.system_namespace != "default"
                && !self.system_namespace.starts_with("kube-"),
            "DedicatedSystemNamespaceRequired"
        );
        if self.cluster_wide {
            ensure!(
                self.application_namespace.is_none(),
                "ClusterScopeApplicationNamespaceConflict"
            );
        } else {
            ensure!(
                self.excluded_namespaces.is_empty(),
                "NamespaceScopeExclusionConflict"
            );
            let app = self
                .application_namespace
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("ApplicationNamespaceRequired"))?;
            ensure!(
                dns_label(app) && !self.exclusions().contains(app),
                "InvalidApplicationNamespace"
            );
        }
        ensure!(
            self.excluded_namespaces.len() <= 64
                && self.excluded_namespaces.iter().all(|ns| dns_label(ns)),
            "InvalidExcludedNamespace"
        );
        Ok(())
    }
    pub fn exclusions(&self) -> BTreeSet<String> {
        let mut excluded = BTreeSet::from([
            self.system_namespace.clone(),
            "kube-system".into(),
            "kube-public".into(),
            "kube-node-lease".into(),
        ]);
        excluded.extend(self.excluded_namespaces.clone());
        excluded
    }
    pub fn allows(&self, namespace: &str) -> bool {
        dns_label(namespace)
            && !self.exclusions().contains(namespace)
            && (self.cluster_wide || self.application_namespace.as_deref() == Some(namespace))
    }
    pub fn webhook_selector(&self) -> Value {
        if self.cluster_wide {
            json!({"matchExpressions":[{"key":"kubernetes.io/metadata.name","operator":"NotIn","values":self.exclusions()}]})
        } else {
            json!({"matchLabels":{"kubernetes.io/metadata.name":self.application_namespace}})
        }
    }
    pub fn registry_namespaces(&self) -> BTreeSet<String> {
        if self.cluster_wide {
            let mut namespaces = BTreeSet::from([
                "*".into(),
                "!kube-system".into(),
                "!kube-public".into(),
                "!kube-node-lease".into(),
            ]);
            namespaces.extend(
                self.excluded_namespaces
                    .iter()
                    .filter(|ns| *ns != &self.system_namespace)
                    .map(|ns| format!("!{ns}")),
            );
            namespaces
        } else {
            let mut namespaces = BTreeSet::from([self.system_namespace.clone()]);
            namespaces.extend(self.application_namespace.clone());
            namespaces
        }
    }
    pub fn overlaps(&self, other: &Self) -> bool {
        if self.cluster_wide && other.cluster_wide {
            return true;
        }
        self.application_namespace
            .as_deref()
            .is_some_and(|ns| other.allows(ns))
            || other
                .application_namespace
                .as_deref()
                .is_some_and(|ns| self.allows(ns))
    }
}
pub fn valid_registry_namespaces(namespaces: &BTreeSet<String>) -> bool {
    !namespaces.is_empty()
        && namespaces.len() <= 128
        && namespaces.iter().all(|ns| {
            ns == "*"
                || ns.strip_prefix('!').map_or_else(
                    || dns_label(ns),
                    |ns| namespaces.contains("*") && dns_label(ns),
                )
        })
}
pub fn registry_allows(namespaces: &BTreeSet<String>, namespace: &str) -> bool {
    dns_label(namespace)
        && (namespaces.contains(namespace) || namespaces.contains("*"))
        && !namespaces.contains(&format!("!{namespace}"))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn namespace_and_cluster_scopes_preserve_system_exclusions_and_repository_boundaries() {
        let local = NamespaceScope {
            system_namespace: "kimage-system".into(),
            application_namespace: Some("apps".into()),
            cluster_wide: false,
            excluded_namespaces: BTreeSet::new(),
        };
        let cluster = NamespaceScope {
            application_namespace: None,
            cluster_wide: true,
            ..local.clone()
        };
        local.validate().unwrap();
        cluster.validate().unwrap();
        assert!(local.allows("apps"));
        assert!(!local.allows("other"));
        assert!(cluster.allows("apps") && cluster.allows("other") && cluster.allows("default"));
        for ns in [
            "kimage-system",
            "kube-system",
            "kube-public",
            "kube-node-lease",
            "*",
            "../apps",
        ] {
            assert!(!cluster.allows(ns));
        }
        assert!(cluster.overlaps(&local));
        assert!(!local.overlaps(&NamespaceScope {
            application_namespace: Some("other".into()),
            ..local.clone()
        }));
        let repositories = cluster.registry_namespaces();
        assert!(valid_registry_namespaces(&repositories));
        assert!(registry_allows(&repositories, "kimage-system"));
        assert!(registry_allows(&repositories, "new-namespace"));
        assert!(!registry_allows(&repositories, "kube-system"));
        assert!(!registry_allows(&repositories, "apps/escape"));
        assert!(!valid_registry_namespaces(&BTreeSet::from(
            ["!apps".into()]
        )));
    }
}

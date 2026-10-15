//! Explicit optional destinations, independent from build declarations.
use anyhow::{Context, Result, ensure};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SecretRef {
    pub name: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Target {
    pub name: String,
    pub registry: Destination,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Destination {
    pub repository: String,
    pub tags: Vec<String>,
    pub credentials_secret_ref: Option<SecretRef>,
    pub ca_secret_ref: Option<SecretRef>,
    #[serde(default)]
    pub unauthenticated: bool,
    #[serde(default)]
    pub overwrite_policy: OverwritePolicy,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub enum OverwritePolicy {
    #[default]
    Never,
}
pub fn tag(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().enumerate().all(|(i, c)| {
            c.is_ascii_alphanumeric() || c == b'_' || (i > 0 && matches!(c, b'.' | b'-'))
        })
}
pub fn repository_component(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut i = 0;
    let alnum = |c: u8| c.is_ascii_lowercase() || c.is_ascii_digit();
    while i < bytes.len() && alnum(bytes[i]) {
        i += 1;
    }
    if i == 0 {
        return false;
    }
    while i < bytes.len() {
        match bytes[i] {
            b'.' => i += 1,
            b'_' => {
                i += 1;
                if i < bytes.len() && bytes[i] == b'_' {
                    i += 1;
                }
            }
            b'-' => {
                while i < bytes.len() && bytes[i] == b'-' {
                    i += 1;
                }
            }
            _ => return false,
        }
        let start = i;
        while i < bytes.len() && alnum(bytes[i]) {
            i += 1;
        }
        if start == i {
            return false;
        }
    }
    true
}
impl Destination {
    pub fn address(&self) -> Result<(reqwest::Url, String, String)> {
        ensure!(
            self.repository.len() <= 253 && self.repository.is_ascii(),
            "InvalidExportRepository"
        );
        let (authority, repository) = self
            .repository
            .split_once('/')
            .context("ExplicitExportAuthorityRequired")?;
        ensure!(
            !repository.is_empty() && repository.split('/').all(repository_component),
            "InvalidExportRepository"
        );
        let origin: reqwest::Url = format!("https://{authority}/").parse()?;
        ensure!(
            origin.host_str().is_some()
                && origin.username().is_empty()
                && origin.password().is_none()
                && origin.path() == "/"
                && origin.query().is_none()
                && origin.fragment().is_none()
                && !authority.contains(['@', '%', '\\'])
                && authority.bytes().all(|c| c.is_ascii_lowercase()
                    || c.is_ascii_digit()
                    || matches!(c, b'.' | b'-' | b':' | b'[' | b']')),
            "InvalidExportAuthority"
        );
        Ok((origin, authority.into(), repository.into()))
    }
    pub fn validate(&self) -> Result<()> {
        self.address()?;
        ensure!(
            !self.tags.is_empty()
                && self.tags.len() <= 16
                && self.tags.iter().all(|s| tag(s))
                && self.tags.iter().collect::<BTreeSet<_>>().len() == self.tags.len(),
            "InvalidExportTags"
        );
        ensure!(
            self.unauthenticated != self.credentials_secret_ref.is_some(),
            "ExplicitExportAuthenticationRequired"
        );
        for reference in self
            .credentials_secret_ref
            .iter()
            .chain(self.ca_secret_ref.iter())
        {
            ensure!(
                kimage_core::selector::dns_subdomain(&reference.name),
                "InvalidExportSecretRef"
            );
        }
        Ok(())
    }
}
pub fn validate(targets: &[Target]) -> Result<()> {
    ensure!(targets.len() <= 8, "TooManyExports");
    let mut names = BTreeSet::new();
    for target in targets {
        ensure!(
            kimage_core::selector::dns_label(&target.name) && names.insert(&target.name),
            "InvalidOrDuplicateExportName"
        );
        target.registry.validate()?;
    }
    Ok(())
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub name: String,
    pub configuration_generation: i64,
    pub configuration_digest: String,
    pub release_uid: Option<String>,
    pub local_digest: Option<String>,
    pub remote_digest: Option<String>,
    pub remote_reference: Option<String>,
    pub phase: String,
    pub attempt_time: String,
    pub attempts: u32,
    pub retry_after: i64,
    pub reason: Option<String>,
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_destination_tags_and_credentials_are_bounded() {
        let mut d = Destination {
            repository: "registry.example:5443/team/api_service".into(),
            tags: vec!["1.2.4".into()],
            credentials_secret_ref: Some(SecretRef {
                name: "push".into(),
            }),
            ca_secret_ref: None,
            unauthenticated: false,
            overwrite_policy: OverwritePolicy::Never,
        };
        assert!(d.validate().is_ok());
        d.unauthenticated = true;
        assert!(d.validate().is_err());
        d.credentials_secret_ref = None;
        assert!(d.validate().is_ok());
        for r in [
            "registry.example",
            "user:password@registry.example/a",
            "registry.example/a/../b",
            "registry.example/a?x=y",
            "registry.example/A",
            "registry.example/a..b",
        ] {
            d.repository = r.into();
            assert!(d.validate().is_err(), "{r}");
        }
        assert!(tag("_release-1.2"));
        assert!(!tag("-invalid"));
        assert!(!tag("a/b"));
    }
}

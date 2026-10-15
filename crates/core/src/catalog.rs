use crate::{
    oci::{Descriptor, Platform, valid_digest},
    selector::{Selection, Selector, exact},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Release {
    pub namespace: String,
    pub image_name: String,
    pub image_uid: String,
    pub uid: String,
    pub resource_version: String,
    pub version: String,
    pub root: Descriptor,
    pub platform: Platform,
    pub snapshot: String,
}
impl Release {
    pub fn repository(&self) -> String {
        format!("{}/{}", self.namespace, self.image_uid)
    }
    pub fn same_content(&self, other: &Self) -> bool {
        self.image_uid == other.image_uid
            && self.version == other.version
            && self.root.digest == other.root.digest
            && self.root.media_type == other.root.media_type
            && self.root.size == other.root.size
            && self.platform == other.platform
    }
}
/// All members come from one bounded capture bracketed by matching Image RVs.
#[derive(Clone, Debug)]
pub struct Catalog {
    pub namespace: String,
    pub name: String,
    pub image_uid: String,
    pub resource_version: String,
    pub channels: BTreeMap<String, String>,
    pub releases: Vec<Release>,
}
impl Catalog {
    pub fn resolve(&self, selector: &Selector, consumer: &str) -> Result<&Release> {
        ensure!(
            selector.namespace == consumer && self.namespace == consumer,
            "CrossNamespaceDenied"
        );
        ensure!(selector.name == self.name, "CatalogIdentityMismatch");
        let selected = match &selector.selection {
            Selection::Exact(v) => Some(v.clone()),
            Selection::Channel(c) => Some(exact(self.channels.get(c).context("UnknownChannel")?)?),
            Selection::Range(_) => None,
        };
        let mut versions = BTreeSet::new();
        let mut candidates = Vec::new();
        for r in &self.releases {
            ensure!(
                r.namespace == self.namespace
                    && r.image_name == self.name
                    && r.image_uid == self.image_uid,
                "CatalogIdentityMismatch"
            );
            ensure!(
                valid_digest(&r.root.digest) && valid_digest(&r.snapshot),
                "InvalidReleaseIntegrity"
            );
            let version = exact(&r.version)?;
            ensure!(
                versions.insert(version.clone()),
                "DuplicatePublishedVersion"
            );
            let eligible = match &selector.selection {
                Selection::Range(req) => req.matches(&version),
                _ => selected.as_ref() == Some(&version),
            };
            if eligible {
                candidates.push((version, r));
            }
        }
        candidates
            .into_iter()
            .max_by(|a, b| a.0.cmp_precedence(&b.0))
            .map(|(_, r)| r)
            .context("NoPublishedRelease")
    }
}
pub fn release_name(uid: &str, version: &str) -> String {
    format!(
        "release-{}",
        &crate::oci::digest(format!("{uid}\0{version}").as_bytes())[7..47]
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    fn catalog() -> Catalog {
        Catalog {
            namespace: "ns".into(),
            name: "app".into(),
            image_uid: "uid".into(),
            resource_version: "4".into(),
            channels: BTreeMap::from([("stable".into(), "1.2.0".into())]),
            releases: ["1.2.0", "1.10.0", "1.11.0-rc.1"]
                .into_iter()
                .map(|version| Release {
                    namespace: "ns".into(),
                    image_name: "app".into(),
                    image_uid: "uid".into(),
                    uid: version.into(),
                    resource_version: "2".into(),
                    version: version.into(),
                    root: crate::oci::descriptor(b"manifest", crate::oci::MANIFEST),
                    platform: Platform::linux_amd64(),
                    snapshot: crate::oci::digest(b"source"),
                })
                .collect(),
        }
    }
    #[test]
    fn exact_channel_range_prerelease_and_recreation() {
        let c = catalog();
        for (uri, v) in [
            ("platform://ns/app:1.2.0", "1.2.0"),
            ("platform://ns/app:stable", "1.2.0"),
            ("platform://ns/app:^1.2.0", "1.10.0"),
            ("platform://ns/app?version=^1.2.0", "1.10.0"),
            ("platform://ns/app?version=%5E1.0.0", "1.10.0"),
            ("platform://ns/app:~1.2.0", "1.2.0"),
            ("platform://ns/app?version=~1.2.0", "1.2.0"),
            ("platform://ns/app:>=1.11.0-rc.1", "1.11.0-rc.1"),
            ("platform://ns/app?version=>=1.11.0-rc.1", "1.11.0-rc.1"),
            ("platform://ns/app?version=%3E=1.11.0-rc.1", "1.11.0-rc.1"),
        ] {
            assert_eq!(
                c.resolve(&Selector::parse(uri).unwrap(), "ns")
                    .unwrap()
                    .version,
                v
            );
        }
        for uri in [
            "platform://ns/app:^2.0.0",
            "platform://ns/app?version=^2.0.0",
            "platform://ns/app?version=%5E2.0.0",
        ] {
            assert!(c.resolve(&Selector::parse(uri).unwrap(), "ns").is_err());
        }
        assert!(
            c.resolve(
                &Selector::parse("platform://ns/app:stable").unwrap(),
                "other"
            )
            .is_err()
        );
        let mut recreated = c.clone();
        recreated.image_uid = "new".into();
        assert!(
            recreated
                .resolve(&Selector::parse("platform://ns/app:stable").unwrap(), "ns")
                .is_err()
        );
        let mut duplicate = c.clone();
        duplicate.releases.push(c.releases[0].clone());
        assert!(
            duplicate
                .resolve(&Selector::parse("platform://ns/app:stable").unwrap(), "ns")
                .is_err()
        );
    }
}

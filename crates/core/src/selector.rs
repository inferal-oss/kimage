//! Grammar: platform://<namespace>/<name>:<exact|channel|range>
//!       or platform://<namespace>/<name>?version=<range>
//! Colon and query selectors are mutually exclusive; no implicit channel exists.
use anyhow::{Result, bail, ensure};
use semver::{Version, VersionReq};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Selection {
    Exact(Version),
    Range(VersionReq),
    Channel(String),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selector {
    pub namespace: String,
    pub name: String,
    pub selection: Selection,
}

pub fn dns_label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value.as_bytes()[value.len() - 1].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}
pub fn dns_subdomain(value: &str) -> bool {
    value.len() <= 253 && value.split('.').all(dns_label)
}
pub fn channel(value: &str) -> bool {
    dns_label(value) && value.as_bytes()[0].is_ascii_lowercase()
}
pub fn exact(value: &str) -> Result<Version> {
    let version = Version::parse(value)?;
    ensure!(
        version.build.is_empty(),
        "version build metadata is unsupported"
    );
    Ok(version)
}
fn parse_range(value: &str) -> Result<Selection> {
    ensure!(
        !value.contains(['%', '+']),
        "double encoding and build metadata unsupported"
    );
    Ok(Selection::Range(VersionReq::parse(value)?))
}
impl Selector {
    pub fn parse(uri: &str) -> Result<Self> {
        ensure!(uri.len() <= 1024, "selector too long");
        let body = uri
            .strip_prefix("platform://")
            .ok_or_else(|| anyhow::anyhow!("expected platform://"))?;
        ensure!(
            !body.contains(['#', '@', '\\']),
            "fragments, userinfo and backslashes are forbidden"
        );
        let (namespace, tail) = body
            .split_once('/')
            .ok_or_else(|| anyhow::anyhow!("missing namespace/name"))?;
        ensure!(
            dns_label(namespace) && !tail.contains('/'),
            "invalid namespace or path"
        );
        let (name, selection) = if let Some((name, query)) = tail.split_once('?') {
            ensure!(!name.contains(':'), "conflicting colon and query selectors");
            let range = query
                .strip_prefix("version=")
                .ok_or_else(|| anyhow::anyhow!("only version query is supported"))?;
            ensure!(
                !range.is_empty() && !range.contains(['&', '?']),
                "invalid or duplicate query"
            );
            for (i, byte) in range.bytes().enumerate() {
                if byte == b'%' {
                    ensure!(
                        range
                            .as_bytes()
                            .get(i + 1)
                            .is_some_and(u8::is_ascii_hexdigit)
                            && range
                                .as_bytes()
                                .get(i + 2)
                                .is_some_and(u8::is_ascii_hexdigit),
                        "malformed percent encoding"
                    );
                }
            }
            let decoded = percent_encoding::percent_decode_str(range).decode_utf8()?;
            (name, parse_range(&decoded)?)
        } else if let Some((name, value)) = tail.split_once(':') {
            ensure!(!value.is_empty(), "empty selector");
            if value.as_bytes()[0].is_ascii_digit() {
                (name, Selection::Exact(exact(value)?))
            } else if value.starts_with(['^', '~', '<', '>', '=', '*']) {
                (name, parse_range(value)?)
            } else {
                ensure!(channel(value), "invalid channel");
                (name, Selection::Channel(value.into()))
            }
        } else {
            bail!("explicit version, range or channel required");
        };
        ensure!(dns_subdomain(name), "invalid Image name");
        Ok(Self {
            namespace: namespace.into(),
            name: name.into(),
            selection,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grammar() {
        for value in [
            "platform://ns/app:1.2.4",
            "platform://ns/app:stable",
            "platform://ns/app:^1.2.0",
            "platform://ns/app:~1.2.0",
            "platform://ns/app?version=^1.2.0",
            "platform://ns/app?version=%5E1.2.0",
            "platform://ns/app?version=~1.2.0",
        ] {
            assert!(Selector::parse(value).is_ok(), "{value}");
        }
        for value in [
            "platform://ns/app:1.2",
            "platform://ns/app:1stable",
            "platform://ns:80/app:stable",
            "platform://ns/app",
            "platform://ns/app:latest?version=*",
            "platform://ns/app?version=*&version=1",
            "platform://ns/app?other=1",
            "platform://ns/app?version=%XX",
            "platform://ns/app?version=%255E1.0.0",
            "platform://ns/app:1.2.3+abc",
            "platform://ns/app:1.2.*",
            "platform://ns/app:1.2.0, <2.0.0",
            "platform://ns/app:^",
            "platform://ns/app:^1.2.0+abc",
            "platform://ns/app:%5E1.2.0",
            "platform://ns/app:^1.2.0?version=^2.0.0",
            "platform://ns/app:^1.2.0&other=1",
            "platform://ns/a/b:stable",
            "platform://ns/app:stable#x",
            "platform://user@ns/app:stable",
        ] {
            assert!(Selector::parse(value).is_err(), "{value}");
        }
    }
    #[test]
    fn exact_is_not_a_range() {
        let selector = Selector::parse("platform://ns/app:1.2.4").unwrap();
        assert_eq!(selector.selection, Selection::Exact(Version::new(1, 2, 4)));
    }
    #[test]
    fn colon_and_query_ranges_are_equivalent() {
        for (readable, encoded) in [
            ("^1.2.0", "%5E1.2.0"),
            ("~1.2.0", "~1.2.0"),
            (">=1.2.0, <2.0.0", "%3E%3D1.2.0%2C%20%3C2.0.0"),
            ("=1.2.0", "%3D1.2.0"),
            ("*", "%2A"),
        ] {
            let colon = Selector::parse(&format!("platform://ns/app:{readable}")).unwrap();
            assert!(matches!(colon.selection, Selection::Range(_)));
            for query in [readable, encoded] {
                assert_eq!(
                    colon,
                    Selector::parse(&format!("platform://ns/app?version={query}")).unwrap()
                );
            }
        }
    }
}

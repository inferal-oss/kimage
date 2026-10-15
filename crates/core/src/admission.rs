//! Read-only admission. Signatures bind metadata to one CREATE request, not users.
use crate::{catalog::Catalog, selector::Selector};
use anyhow::{Context, Result, bail, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;
use std::collections::{BTreeMap, BTreeSet};
pub const RESOLUTIONS: &str = "kimage.dev/resolutions";
pub const SIGNATURE: &str = "kimage.dev/resolution-signature";
/// Loopback aliases cannot bypass the direct internal-reference policy.
pub fn internal_reference(image: &str, endpoint: &str) -> bool {
    let Some((authority, _)) = image.split_once('/') else {
        return false;
    };
    let Ok(expected) = endpoint.parse::<std::net::SocketAddr>() else {
        return false;
    };
    if let Ok(address) = authority.parse::<std::net::SocketAddr>() {
        let loopback = address.ip().is_loopback()
            || matches!(address.ip(), std::net::IpAddr::V6(ip) if ip.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback()));
        return address.port() == expected.port() && loopback;
    }
    authority.rsplit_once(':').is_some_and(|(host, port)| {
        host.trim_end_matches('.').eq_ignore_ascii_case("localhost")
            && port.parse::<u16>().ok() == Some(expected.port())
    })
}

pub struct Policy<'a> {
    pub namespace: &'a str,
    pub endpoint: &'a str,
    pub request_uid: &'a str,
    pub key: &'a [u8],
    pub nodes: &'a BTreeSet<String>,
    pub node_info: Option<&'a BTreeMap<String, NodeInfo>>,
}
#[derive(Clone, Debug)]
pub struct NodeInfo {
    pub platform: crate::oci::Platform,
    pub labels: BTreeMap<String, String>,
}
fn node_platform(policy: &Policy<'_>, name: &str) -> crate::oci::Platform {
    policy
        .node_info
        .and_then(|nodes| nodes.get(name))
        .map(|n| n.platform.clone())
        .unwrap_or_else(crate::oci::Platform::linux_amd64)
}
fn matches_requirement(requirement: &Value, value: Option<&str>) -> Result<bool> {
    let values = requirement["values"].as_array();
    Ok(match requirement["operator"].as_str() {
        Some("In") => {
            value.is_some_and(|value| values.is_some_and(|v| v.iter().any(|v| v == value)))
        }
        Some("NotIn") => {
            !value.is_some_and(|value| values.is_some_and(|v| v.iter().any(|v| v == value)))
        }
        Some("Exists") => value.is_some(),
        Some("DoesNotExist") => value.is_none(),
        Some("Gt" | "Lt") => {
            let limit = values
                .filter(|v| v.len() == 1)
                .and_then(|v| v[0].as_str())
                .and_then(|v| v.parse::<i64>().ok())
                .context("InvalidNumericNodeSelector")?;
            value.and_then(|v| v.parse::<i64>().ok()).is_some_and(|v| {
                if requirement["operator"] == "Gt" {
                    v > limit
                } else {
                    v < limit
                }
            })
        }
        _ => bail!("UnsupportedNodeSelectorOperator"),
    })
}
/// Infer requested platforms from actual eligible nodes and immutable workload
/// placement constraints. Unknown placement retains every possible platform.
pub fn placement_nodes(pod: &Value, policy: &Policy<'_>) -> Result<BTreeSet<String>> {
    ensure!(
        pod["spec"]["os"]["name"]
            .as_str()
            .is_none_or(|os| os == "linux"),
        "UnsupportedPodOperatingSystem"
    );
    let mut selected = BTreeSet::new();
    for name in policy.nodes {
        if pod["spec"]["nodeName"]
            .as_str()
            .is_some_and(|node| node != name)
        {
            continue;
        }
        let platform = node_platform(policy, name);
        if !platform.supported() {
            continue;
        }
        let mut labels = policy
            .node_info
            .and_then(|nodes| nodes.get(name))
            .map(|n| n.labels.clone())
            .unwrap_or_default();
        labels.insert("kubernetes.io/os".into(), platform.os.clone());
        labels.insert("kubernetes.io/arch".into(), platform.architecture.clone());
        if let Some(selectors) = pod["spec"]["nodeSelector"].as_object()
            && selectors.iter().any(|(k, v)| {
                (policy.node_info.is_some()
                    || matches!(k.as_str(), "kubernetes.io/os" | "kubernetes.io/arch"))
                    && v.as_str() != labels.get(k).map(String::as_str)
            })
        {
            continue;
        }
        if let Some(terms) = pod["spec"]["affinity"]["nodeAffinity"]["requiredDuringSchedulingIgnoredDuringExecution"]["nodeSelectorTerms"].as_array() {
            let mut matches = false;
            for term in terms {
                let mut nonempty = false;
                let mut eligible = true;
                for (field, fields) in [("matchExpressions", false), ("matchFields", true)] {
                    for r in term[field].as_array().into_iter().flatten() {
                        nonempty = true;
                        let key = r["key"].as_str().context("InvalidNodeSelectorKey")?;
                        if !fields && policy.node_info.is_none() && !matches!(key, "kubernetes.io/os" | "kubernetes.io/arch") { continue; }
                        let value = if fields { if key == "metadata.name" { Some(name.as_str()) } else { bail!("UnsupportedNodeSelectorField"); } } else {labels.get(key).map(String::as_str)};
                        eligible &= matches_requirement(r, value)?;
                    }
                }
                matches |= eligible && nonempty;
            }
            if !matches { continue; }
        }
        selected.insert(name.clone());
    }
    Ok(selected)
}
pub fn requested_platforms(
    pod: &Value,
    policy: &Policy<'_>,
    catalogs: &BTreeMap<String, Catalog>,
) -> Result<BTreeSet<crate::oci::Platform>> {
    let mut platforms: BTreeSet<_> = placement_nodes(pod, policy)?
        .iter()
        .map(|n| node_platform(policy, n))
        .collect();
    for kind in ["containers", "initContainers", "ephemeralContainers"] {
        for container in pod["spec"][kind].as_array().into_iter().flatten() {
            let Some(uri) = container["image"]
                .as_str()
                .filter(|s| s.starts_with("platform://"))
            else {
                continue;
            };
            let selector = Selector::parse(uri)?;
            ensure!(
                selector.namespace == policy.namespace,
                "CrossNamespaceDenied"
            );
            let catalog = catalogs.get(&selector.name).context("UnknownImage")?;
            let declared = if catalog.platforms.is_empty() {
                catalog
                    .releases
                    .iter()
                    .flat_map(|r| r.platforms())
                    .collect()
            } else {
                catalog.platforms.clone()
            };
            if !declared.is_empty() {
                platforms.retain(|p| declared.contains(p));
            }
        }
    }
    ensure!(!platforms.is_empty(), "NoCompatibleImagePlatform");
    Ok(platforms)
}
fn signature(policy: &Policy<'_>, bytes: &[u8]) -> Hmac<Sha256> {
    let mut mac = Hmac::<Sha256>::new_from_slice(policy.key).expect("HMAC key");
    mac.update(policy.namespace.as_bytes());
    mac.update(&[0]);
    mac.update(policy.request_uid.as_bytes());
    mac.update(&[0]);
    mac.update(bytes);
    mac
}
pub fn patch(
    pod: &Value,
    policy: &Policy<'_>,
    catalogs: &BTreeMap<String, Catalog>,
) -> Result<Vec<Value>> {
    ensure!(policy.key.len() >= 32, "InvalidAdmissionKey");
    let authority: std::net::SocketAddr = policy.endpoint.parse().context("InvalidEndpoint")?;
    ensure!(
        authority.ip() == std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
            && authority.port() >= 1024,
        "InvalidEndpoint"
    );
    ensure!(
        pod["metadata"].is_object() && pod["spec"].is_object(),
        "InvalidPod"
    );
    let annotations = &pod["metadata"]["annotations"];
    let has_resolution = annotations.get(RESOLUTIONS).is_some();
    let has_signature = annotations.get(SIGNATURE).is_some();
    ensure!(
        has_resolution == has_signature,
        "UntrustedResolutionMetadata"
    );
    let mut recorded: BTreeMap<(String, String), Value> = BTreeMap::new();
    if has_resolution {
        let text = annotations[RESOLUTIONS]
            .as_str()
            .context("InvalidResolutionMetadata")?;
        ensure!(text.len() <= 16384, "ResolutionMetadataTooLarge");
        let sig = STANDARD.decode(
            annotations[SIGNATURE]
                .as_str()
                .context("InvalidSignature")?,
        )?;
        signature(policy, text.as_bytes())
            .verify_slice(&sig)
            .context("UntrustedResolutionMetadata")?;
        let entries: Vec<Value> = serde_json::from_str(text)?;
        ensure!(
            !entries.is_empty() && entries.len() <= 128,
            "InvalidResolutionMetadata"
        );
        for e in entries {
            let kind = e["kind"]
                .as_str()
                .context("InvalidContainerKind")?
                .to_owned();
            let name = e["container"]
                .as_str()
                .context("InvalidContainerName")?
                .to_owned();
            ensure!(
                recorded.insert((kind, name), e).is_none(),
                "DuplicateResolutionMetadata"
            );
        }
    }
    let mut result = Vec::new();
    let mut records = Vec::new();
    let mut eligible = policy.nodes.clone();
    let mut placed: Option<BTreeSet<String>> = None;
    let mut requested = None;
    for kind in ["containers", "initContainers", "ephemeralContainers"] {
        if let Some(items) = pod["spec"][kind].as_array() {
            ensure!(items.len() <= 64, "TooManyContainers");
            for (index, c) in items.iter().enumerate() {
                let image = c["image"].as_str().context("MissingImage")?;
                let name = c["name"].as_str().context("MissingContainerName")?;
                let managed =
                    image.starts_with("platform://") || internal_reference(image, policy.endpoint);
                if !managed {
                    ensure!(
                        !recorded.contains_key(&(kind.into(), name.into())),
                        "InconsistentResolutionMetadata"
                    );
                    continue;
                }
                let placement = match &placed {
                    Some(nodes) => nodes,
                    None => placed.insert(placement_nodes(pod, policy)?),
                };
                ensure!(!placement.is_empty(), "RuntimeEndpointUnavailable");
                eligible.retain(|n| placement.contains(n));
                ensure!(
                    kind != "ephemeralContainers",
                    "UnsupportedEphemeralReference"
                );
                if let Some(record) = recorded.remove(&(kind.into(), name.into())) {
                    let selector =
                        Selector::parse(record["selector"].as_str().context("InvalidSelector")?)?;
                    ensure!(
                        selector.namespace == policy.namespace,
                        "CrossNamespaceDenied"
                    );
                    let catalog = catalogs.get(&selector.name).context("UnknownImage")?;
                    let release = catalog
                        .releases
                        .iter()
                        .find(|r| r.uid == record["releaseUID"] && r.image_uid == catalog.image_uid)
                        .context("UnpublishedResolution")?;
                    ensure!(
                        record["digest"] == release.root.digest
                            && record["version"] == release.version
                            && record["imageUID"] == release.image_uid
                            && record["resourceVersion"] == release.resource_version,
                        "InconsistentResolutionMetadata"
                    );
                    ensure!(
                        image
                            == format!(
                                "{}/{}@{}",
                                policy.endpoint,
                                release.repository(),
                                release.root.digest
                            )
                            && c["imagePullPolicy"] == "IfNotPresent",
                        "InconsistentResolutionMetadata"
                    );
                    eligible.retain(|n| release.platforms().contains(&node_platform(policy, n)));
                    records.push(record);
                    continue;
                }
                ensure!(
                    image.starts_with("platform://"),
                    "DirectInternalReferenceDenied"
                );
                let selector = Selector::parse(image)?;
                let catalog = catalogs.get(&selector.name).context("UnknownImage")?;
                let platforms = match &requested {
                    Some(p) => p,
                    None => requested.insert(requested_platforms(pod, policy, catalogs)?),
                };
                let release = catalog.resolve_for(&selector, policy.namespace, platforms)?;
                eligible.retain(|n| release.platforms().contains(&node_platform(policy, n)));
                result.push(json!({"op":"replace","path":format!("/spec/{kind}/{index}/image"),"value":format!("{}/{}@{}",policy.endpoint,release.repository(),release.root.digest)}));
                result.push(json!({"op":if c.get("imagePullPolicy").is_some(){"replace"}else{"add"},"path":format!("/spec/{kind}/{index}/imagePullPolicy"),"value":"IfNotPresent"}));
                records.push(json!({"kind":kind,"container":name,"selector":image,"releaseUID":release.uid,"imageUID":release.image_uid,"version":release.version,"digest":release.root.digest,"resourceVersion":release.resource_version,"catalogResourceVersion":catalog.resource_version}));
            }
        }
    }
    ensure!(recorded.is_empty(), "MissingResolvedContainer");
    if records.is_empty() {
        return Ok(result);
    }
    ensure!(!eligible.is_empty(), "IncompatibleImagePlatforms");
    ensure!(
        pod["spec"]["nodeSelector"]["kubernetes.io/os"]
            .as_str()
            .is_none_or(|v| v == "linux"),
        "IncompatiblePlatformPlacement"
    );
    ensure!(
        pod["spec"]["schedulerName"]
            .as_str()
            .is_none_or(|s| s == "default-scheduler"),
        "UnsupportedScheduler"
    );
    ensure!(
        pod["metadata"]["annotations"]
            .get("kubernetes.io/config.mirror")
            .is_none(),
        "UnsupportedMirrorPod"
    );
    if let Some(node) = pod["spec"]["nodeName"].as_str() {
        ensure!(eligible.contains(node), "NodeEndpointUnavailable");
    }
    let affinity = eligible_affinity_for(&pod["spec"]["affinity"], &eligible, policy.node_info)?;
    if pod["spec"]["affinity"] != affinity {
        result.push(json!({"op":if pod["spec"].get("affinity").is_some(){"replace"}else{"add"},"path":"/spec/affinity","value":affinity}));
    }
    let text = serde_json::to_string(&records)?;
    ensure!(text.len() <= 16384, "ResolutionMetadataTooLarge");
    let sig = STANDARD.encode(signature(policy, text.as_bytes()).finalize().into_bytes());
    if !annotations.is_object() {
        result.push(json!({"op":"add","path":"/metadata/annotations","value":{RESOLUTIONS:text,SIGNATURE:sig}}));
    } else {
        for (key, value) in [(RESOLUTIONS, text), (SIGNATURE, sig)] {
            if annotations[key] != value {
                result.push(json!({"op":"add","path":format!("/metadata/annotations/{}",key.replace('~',"~0").replace('/',"~1")),"value":value}));
            }
        }
    }
    Ok(result)
}
#[cfg(test)]
fn eligible_affinity(existing: &Value, nodes: &BTreeSet<String>) -> Result<Value> {
    eligible_affinity_for(existing, nodes, None)
}
fn eligible_affinity_for(
    existing: &Value,
    nodes: &BTreeSet<String>,
    node_info: Option<&BTreeMap<String, NodeInfo>>,
) -> Result<Value> {
    let was_required =
        !existing["nodeAffinity"]["requiredDuringSchedulingIgnoredDuringExecution"].is_null();
    let mut value = if existing.is_null() {
        json!({})
    } else {
        existing.clone()
    };
    ensure!(value.is_object(), "InvalidAffinity");
    let node = value
        .as_object_mut()
        .unwrap()
        .entry("nodeAffinity")
        .or_insert(json!({}));
    ensure!(node.is_object(), "InvalidNodeAffinity");
    let required = node
        .as_object_mut()
        .unwrap()
        .entry("requiredDuringSchedulingIgnoredDuringExecution")
        .or_insert(json!({"nodeSelectorTerms":[{}]}));
    let terms = required["nodeSelectorTerms"]
        .as_array_mut()
        .context("InvalidNodeSelectorTerms")?;
    ensure!(!terms.is_empty(), "UnschedulableEmptyAffinity");
    let original = std::mem::take(terms);
    ensure!(
        original.len().saturating_mul(nodes.len()) <= 256,
        "AffinityExpansionTooLarge"
    );
    for term in original {
        // An existing empty term matches no nodes; preserve that semantics.
        if term.as_object().is_none() {
            bail!("InvalidNodeSelectorTerm");
        }
        if was_required
            && !term["matchExpressions"]
                .as_array()
                .is_some_and(|a| !a.is_empty())
            && !term["matchFields"]
                .as_array()
                .is_some_and(|a| !a.is_empty())
        {
            terms.push(term);
            continue;
        }
        // Field selectors accept one value. Distribute node eligibility over
        // the user's OR terms, preserving every original conjunction.
        for node_name in nodes {
            if term["matchFields"].as_array().is_some_and(|fields| {
                fields.iter().any(|f| {
                    f["key"] == "metadata.name"
                        && match f["operator"].as_str() {
                            Some("In") => !f["values"]
                                .as_array()
                                .is_some_and(|v| v.iter().any(|v| v == node_name)),
                            Some("NotIn") => f["values"]
                                .as_array()
                                .is_some_and(|v| v.iter().any(|v| v == node_name)),
                            _ => false,
                        }
                })
            }) {
                continue;
            }
            let mut term = term.clone();
            let expr = json!({"key":"metadata.name","operator":"In","values":[node_name]});
            let fields = term
                .as_object_mut()
                .unwrap()
                .entry("matchFields")
                .or_insert(json!([]))
                .as_array_mut()
                .context("InvalidMatchFields")?;
            if !fields.contains(&expr) {
                fields.push(expr);
            }
            let expressions = term
                .as_object_mut()
                .unwrap()
                .entry("matchExpressions")
                .or_insert(json!([]))
                .as_array_mut()
                .context("InvalidMatchExpressions")?;
            let architecture = node_info
                .and_then(|n| n.get(node_name))
                .map(|n| n.platform.architecture.as_str())
                .unwrap_or("amd64");
            for (key, v) in [
                ("kubernetes.io/os", "linux"),
                ("kubernetes.io/arch", architecture),
            ] {
                let expr = json!({"key":key,"operator":"In","values":[v]});
                if !expressions.contains(&expr) {
                    expressions.push(expr);
                }
            }
            terms.push(term);
        }
    }
    ensure!(!terms.is_empty(), "NoEligibleAffinityNodes");
    Ok(value)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        catalog::Release,
        oci::{MANIFEST, Platform, descriptor, digest},
    };
    #[test]
    fn native_variants_follow_placement_and_unconstrained_pods_require_the_index() {
        use crate::oci::Platform;
        let node_info = BTreeMap::from([
            (
                "x86".into(),
                NodeInfo {
                    platform: Platform::linux_amd64(),
                    labels: BTreeMap::from([("zone".into(), "west".into())]),
                },
            ),
            (
                "arm".into(),
                NodeInfo {
                    platform: Platform::linux_arm64(),
                    labels: BTreeMap::from([("zone".into(), "east".into())]),
                },
            ),
        ]);
        let nodes = node_info.keys().cloned().collect();
        let policy = Policy {
            namespace: "ns",
            endpoint: "127.0.0.1:17500",
            request_uid: "placement",
            key: &[7; 32],
            nodes: &nodes,
            node_info: Some(&node_info),
        };
        let releases = [
            Platform::linux_amd64(),
            Platform::linux_arm64(),
            Platform::automatic(),
        ]
        .into_iter()
        .map(|platform| Release {
            namespace: "ns".into(),
            image_name: "app".into(),
            image_uid: "uid".into(),
            uid: platform.architecture.clone(),
            resource_version: "1".into(),
            version: "1.0.0".into(),
            snapshot: digest(b"source"),
            root: descriptor(
                platform.architecture.as_bytes(),
                if platform.is_automatic() {
                    crate::oci::INDEX
                } else {
                    MANIFEST
                },
            ),
            platform,
        })
        .collect();
        let catalogs = BTreeMap::from([(
            "app".into(),
            Catalog {
                namespace: "ns".into(),
                name: "app".into(),
                image_uid: "uid".into(),
                resource_version: "1".into(),
                channels: BTreeMap::new(),
                platforms: BTreeSet::from([Platform::linux_amd64(), Platform::linux_arm64()]),
                releases,
            },
        )]);
        for (placement, architecture, expected_nodes) in [
            (
                json!({}),
                "auto",
                BTreeSet::from(["arm".to_owned(), "x86".to_owned()]),
            ),
            (
                json!({"nodeSelector":{"kubernetes.io/arch":"arm64"}}),
                "arm64",
                BTreeSet::from(["arm".to_owned()]),
            ),
            (
                json!({"nodeName":"x86"}),
                "amd64",
                BTreeSet::from(["x86".to_owned()]),
            ),
            (
                json!({"nodeSelector":{"zone":"east"}}),
                "arm64",
                BTreeSet::from(["arm".to_owned()]),
            ),
            (
                json!({"affinity":{"nodeAffinity":{"requiredDuringSchedulingIgnoredDuringExecution":{"nodeSelectorTerms":[{"matchExpressions":[{"key":"kubernetes.io/arch","operator":"In","values":["amd64"]}]}]}}}}),
                "amd64",
                BTreeSet::from(["x86".to_owned()]),
            ),
        ] {
            let mut pod = json!({"metadata":{},"spec":{"containers":[{"name":"app","image":"platform://ns/app:1.0.0"}]}});
            pod["spec"]
                .as_object_mut()
                .unwrap()
                .extend(placement.as_object().unwrap().clone());
            let expected_platforms: BTreeSet<_> = expected_nodes
                .iter()
                .map(|n| node_info[n].platform.clone())
                .collect();
            assert_eq!(
                requested_platforms(&pod, &policy, &catalogs).unwrap(),
                expected_platforms
            );
            let changes: json_patch::Patch =
                serde_json::from_value(json!(patch(&pod, &policy, &catalogs).unwrap())).unwrap();
            json_patch::patch(&mut pod, &changes).unwrap();
            assert_eq!(
                pod["spec"]["containers"][0]["image"],
                format!("127.0.0.1:17500/ns/uid@{}", digest(architecture.as_bytes()))
            );
            let placed = placement_nodes(&pod, &policy).unwrap();
            assert_eq!(placed, expected_nodes);
            assert!(patch(&pod, &policy, &catalogs).unwrap().is_empty());
        }
        let mut incomplete = catalogs.clone();
        incomplete
            .get_mut("app")
            .unwrap()
            .releases
            .retain(|r| !r.platform.is_automatic());
        let pod = json!({"metadata":{},"spec":{"containers":[{"name":"app","image":"platform://ns/app:1.0.0"}]}});
        assert!(patch(&pod, &policy, &incomplete).is_err());
        let mut incompatible = pod;
        incompatible["spec"]["nodeSelector"] = json!({"kubernetes.io/arch":"s390x"});
        assert!(patch(&incompatible, &policy, &catalogs).is_err());
        incompatible["spec"]
            .as_object_mut()
            .unwrap()
            .remove("nodeSelector");
        incompatible["spec"]["os"] = json!({"name":"windows"});
        assert!(patch(&incompatible, &policy, &catalogs).is_err());
        assert_ne!(
            crate::catalog::platform_release_name("uid", "1.0.0", &Platform::linux_amd64()),
            crate::catalog::platform_release_name("uid", "1.0.0", &Platform::linux_arm64())
        );
    }
    #[test]
    fn readable_and_encoded_ranges_pin_normal_and_init_containers() {
        let catalog = Catalog {
            namespace: "ns".into(),
            name: "app".into(),
            image_uid: "uid".into(),
            resource_version: "1".into(),
            channels: BTreeMap::new(),
            platforms: BTreeSet::new(),
            releases: ["1.2.0", "1.9.0", "2.0.0", "1.10.0-rc.1"]
                .into_iter()
                .map(|version| Release {
                    namespace: "ns".into(),
                    image_name: "app".into(),
                    image_uid: "uid".into(),
                    uid: version.into(),
                    resource_version: "2".into(),
                    version: version.into(),
                    root: descriptor(version.as_bytes(), MANIFEST),
                    platform: Platform::linux_amd64(),
                    snapshot: digest(b"snapshot"),
                })
                .collect(),
        };
        let expected = format!("127.0.0.1:17500/ns/uid@{}", catalog.releases[1].root.digest);
        let catalogs = BTreeMap::from([("app".into(), catalog)]);
        let nodes = BTreeSet::from(["node".into()]);
        let policy = Policy {
            namespace: "ns",
            endpoint: "127.0.0.1:17500",
            request_uid: "range-request",
            key: &[7; 32],
            nodes: &nodes,
            node_info: None,
        };
        for selector in [
            "platform://ns/app:^1.2.0",
            "platform://ns/app?version=^1.2.0",
            "platform://ns/app?version=%5E1.2.0",
        ] {
            let mut pod = json!({"metadata":{},"spec":{
                "containers":[{"name":"app","image":selector}],
                "initContainers":[{"name":"init","image":selector}]
            }});
            let changes: json_patch::Patch =
                serde_json::from_value(json!(patch(&pod, &policy, &catalogs).unwrap())).unwrap();
            json_patch::patch(&mut pod, &changes).unwrap();
            for kind in ["containers", "initContainers"] {
                assert_eq!(pod["spec"][kind][0]["image"], expected);
                assert_eq!(pod["spec"][kind][0]["imagePullPolicy"], "IfNotPresent");
            }
            let records: Vec<Value> = serde_json::from_str(
                pod["metadata"]["annotations"][RESOLUTIONS]
                    .as_str()
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(records.len(), 2);
            for record in records {
                assert_eq!(record["selector"], selector);
                assert_eq!(record["version"], "1.9.0");
            }
            assert!(patch(&pod, &policy, &catalogs).unwrap().is_empty());
        }
    }
    #[test]
    fn patch_application_reinvocation_forgery_and_conjunction() {
        let catalogs = BTreeMap::from([(
            "app".into(),
            Catalog {
                namespace: "ns".into(),
                name: "app".into(),
                image_uid: "uid".into(),
                resource_version: "1".into(),
                channels: BTreeMap::new(),
                platforms: BTreeSet::new(),
                releases: vec![Release {
                    namespace: "ns".into(),
                    image_name: "app".into(),
                    image_uid: "uid".into(),
                    uid: "r".into(),
                    resource_version: "2".into(),
                    version: "1.0.0".into(),
                    root: descriptor(b"m", MANIFEST),
                    platform: Platform::linux_amd64(),
                    snapshot: digest(b"s"),
                }],
            },
        )]);
        let nodes = BTreeSet::from(["node".into()]);
        let key = [7; 32];
        let policy = Policy {
            namespace: "ns",
            endpoint: "127.0.0.1:17500",
            request_uid: "request",
            key: &key,
            nodes: &nodes,
            node_info: None,
        };
        let mut pod = json!({"metadata":{"name":"p"},"spec":{"containers":[{"name":"app","image":"platform://ns/app:1.0.0"},{"name":"other","image":"example.org/plain","imagePullPolicy":"Always"}],"initContainers":[{"name":"init","image":"platform://ns/app:1.0.0","restartPolicy":"Always"}],"affinity":{"nodeAffinity":{"requiredDuringSchedulingIgnoredDuringExecution":{"nodeSelectorTerms":[{"matchExpressions":[{"key":"zone","operator":"In","values":["a"]}]},{"matchFields":[{"key":"metadata.name","operator":"In","values":["node"]}]}]}}}}});
        let p: json_patch::Patch =
            serde_json::from_value(json!(patch(&pod, &policy, &catalogs).unwrap())).unwrap();
        json_patch::patch(&mut pod, &p).unwrap();
        assert_eq!(pod["spec"]["containers"][1]["imagePullPolicy"], "Always");
        for t in pod["spec"]["affinity"]["nodeAffinity"]["requiredDuringSchedulingIgnoredDuringExecution"]["nodeSelectorTerms"].as_array().unwrap(){assert!(!t["matchFields"].as_array().unwrap().is_empty());}
        assert!(patch(&pod, &policy, &catalogs).unwrap().is_empty());
        let other = Policy {
            request_uid: "forged",
            ..policy
        };
        assert!(patch(&pod, &other, &catalogs).is_err());
        pod["spec"]["containers"][0]["image"] = json!("127.0.0.1:17500/ns/uid@sha256:bad");
        assert!(patch(&pod, &other, &catalogs).is_err());
    }
    #[test]
    fn unmanaged_pod_has_no_patch() {
        let nodes = BTreeSet::new();
        let key = [0; 32];
        assert!(patch(&json!({"metadata":{},"spec":{"containers":[{"name":"p","image":"example.org/a:v1"}]}}),&Policy{namespace:"ns",endpoint:"127.0.0.1:17500",request_uid:"r",key:&key,nodes:&nodes,node_info:None},&BTreeMap::new()).unwrap().is_empty());
    }
    #[test]
    fn loopback_aliases_cannot_bypass_direct_reference_policy() {
        for host in [
            "localhost:17500",
            "LOCALHOST.:17500",
            "127.0.0.1:17500",
            "[::ffff:127.0.0.1]:17500",
        ] {
            let image = format!("{host}/ns/uid@{}", crate::oci::digest(b"root"));
            let pod = json!({"metadata":{},"spec":{"containers":[{"name":"app","image":image}]}});
            assert!(
                patch(
                    &pod,
                    &Policy {
                        namespace: "ns",
                        endpoint: "127.0.0.1:17500",
                        request_uid: "review",
                        key: &[0; 32],
                        node_info: None,
                        nodes: &BTreeSet::new()
                    },
                    &BTreeMap::new()
                )
                .is_err()
            );
        }
        assert!(!internal_reference(
            "localhost:18000/ns/a:v1",
            "127.0.0.1:17500"
        ));
        assert!(!internal_reference(
            "example.org:17500/ns/a:v1",
            "127.0.0.1:17500"
        ));
    }
    #[test]
    fn multiple_nodes_use_single_value_fields_and_preserve_user_or_terms() {
        let nodes = BTreeSet::from(["a".into(), "b".into()]);
        let existing = json!({"nodeAffinity":{"requiredDuringSchedulingIgnoredDuringExecution":{"nodeSelectorTerms":[{"matchExpressions":[{"key":"zone","operator":"In","values":["east"]}]},{"matchFields":[{"key":"metadata.name","operator":"NotIn","values":["a"]}]}]}}});
        let patched = eligible_affinity(&existing, &nodes).unwrap();
        let terms = patched["nodeAffinity"]["requiredDuringSchedulingIgnoredDuringExecution"]["nodeSelectorTerms"].as_array().unwrap();
        assert_eq!(terms.len(), 3);
        for term in terms {
            for field in term["matchFields"].as_array().unwrap() {
                assert_eq!(field["values"].as_array().unwrap().len(), 1);
            }
        }
        assert_eq!(eligible_affinity(&patched, &nodes).unwrap(), patched);
        assert!(eligible_affinity(&existing, &BTreeSet::from(["c".into()])).is_ok());
    }
    #[test]
    fn empty_required_affinity_terms_never_become_matching_terms() {
        let nodes = BTreeSet::from(["node".into()]);
        let existing = json!({"nodeAffinity":{"requiredDuringSchedulingIgnoredDuringExecution":{"nodeSelectorTerms":[{}, {"matchFields":[],"matchExpressions":[]}, {"matchExpressions":[{"key":"zone","operator":"In","values":["a"]}]}]}}});
        let patched = eligible_affinity(&existing, &nodes).unwrap();
        let terms = &patched["nodeAffinity"]["requiredDuringSchedulingIgnoredDuringExecution"]["nodeSelectorTerms"];
        assert_eq!(terms[0], json!({}));
        assert_eq!(terms[1], json!({"matchFields":[],"matchExpressions":[]}));
        assert_eq!(terms[2]["matchFields"][0]["key"], "metadata.name");
    }
}

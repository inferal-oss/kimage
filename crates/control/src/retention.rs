//! Alpha's durable release pin is permanent. No artifact or metadata GC runs.
//! An administrator bypassing admission/finalizers is outside this guarantee.
use anyhow::{Result, ensure};
use serde_json::Value;

pub const PIN: &str = "kimage.dev/retained";
fn pinned(object: &Value) -> bool {
    object["metadata"]["finalizers"]
        .as_array()
        .is_some_and(|items| items.iter().any(|v| v == PIN))
}
pub fn validate(request: &Value, namespace: &str, operator: Option<&str>) -> Result<()> {
    ensure!(
        request["uid"].as_str().is_some_and(|v| !v.is_empty()),
        "MissingAdmissionUid"
    );
    ensure!(
        request["namespace"] == namespace
            && request["resource"]["group"] == "kimage.dev"
            && request["resource"]["resource"] == "imagereleases",
        "RetentionScopeMismatch"
    );
    if request["operation"] == "CREATE" {
        ensure!(
            operator.is_some_and(|name| request["userInfo"]["username"] == name)
                && pinned(&request["object"]),
            "ReleasePublicationIdentityRequired"
        );
        return Ok(());
    }
    let old = &request["oldObject"];
    ensure!(
        old["metadata"]["uid"]
            .as_str()
            .is_some_and(|v| !v.is_empty()),
        "MissingReleaseIdentity"
    );
    let published = old["status"]["verified"] == true;
    let publisher = operator.is_some_and(|name| request["userInfo"]["username"] == name);
    match request["operation"].as_str() {
        Some("DELETE") => ensure!(
            !published && (!pinned(old) || publisher),
            "RetainedReleasePinned"
        ),
        Some("UPDATE") => {
            if pinned(old) && !pinned(&request["object"]) {
                ensure!(!published && publisher, "RetainedReleasePinned");
            }
        }
        _ => anyhow::bail!("UnsupportedRetentionOperation"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn published_pins_cannot_be_removed_or_deleted_even_by_publisher() {
        let identity = "system:serviceaccount:system:operator";
        let mut request = json!({"uid":"review","namespace":"app","resource":{"group":"kimage.dev","resource":"imagereleases"},"operation":"DELETE","userInfo":{"username":identity},"oldObject":{"metadata":{"uid":"release","finalizers":[PIN]},"status":{"verified":true}}});
        assert!(validate(&request, "app", Some(identity)).is_err());
        request["operation"] = json!("UPDATE");
        request["object"] = request["oldObject"].clone();
        assert!(validate(&request, "app", Some(identity)).is_ok());
        request["object"]["metadata"]["finalizers"] = json!([]);
        assert!(validate(&request, "app", Some(identity)).is_err());
        request["operation"] = json!("DELETE");
        request["oldObject"]["status"]["verified"] = json!(false);
        assert!(validate(&request, "app", Some(identity)).is_ok());
        request["userInfo"]["username"] = json!("author");
        assert!(validate(&request, "app", Some(identity)).is_err());
        assert!(validate(&request, "other", Some(identity)).is_err());
    }
}

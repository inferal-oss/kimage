//! Bounded optional export declarations; no change to stored release identity.
use serde_json::{Value, json};
pub fn apply(spec: &mut Value) {
    let exports = &mut spec["properties"]["exports"];
    // Adding an optional feature must not default new spec data into retained
    // Images merely because their CRD was expanded.
    exports.as_object_mut().unwrap().remove("default");
    exports["maxItems"] = json!(8);
    exports["x-kubernetes-list-type"] = json!("map");
    exports["x-kubernetes-list-map-keys"] = json!(["name"]);
    let target = &mut exports["items"]["properties"];
    target["name"]["maxLength"] = json!(63);
    target["name"]["pattern"] = json!("^[a-z0-9]([-a-z0-9]*[a-z0-9])?$");
    let registry = &mut target["registry"];
    registry["x-kubernetes-validations"] = json!([{"rule":"has(self.credentialsSecretRef) != self.unauthenticated","message":"Select explicit same-namespace credentials or unauthenticated export"}]);
    let p = &mut registry["properties"];
    p["repository"]["maxLength"] = json!(253);
    p["repository"]["pattern"] = json!(
        r"^([a-z0-9][a-z0-9.-]*(:[0-9]{1,5})?|\[[0-9a-f:]+\](:[0-9]{1,5})?)/[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*(/[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*)*$"
    );
    p["tags"]["minItems"] = json!(1);
    p["tags"]["maxItems"] = json!(16);
    p["tags"]["x-kubernetes-list-type"] = json!("set");
    p["tags"]["items"]["maxLength"] = json!(128);
    p["tags"]["items"]["pattern"] = json!("^[a-zA-Z0-9_][a-zA-Z0-9_.-]{0,127}$");
    for field in ["credentialsSecretRef", "caSecretRef"] {
        p[field]["properties"]["name"]["maxLength"] = json!(253);
        p[field]["properties"]["name"]["pattern"] = json!("^[a-z0-9]([-a-z0-9.]*[a-z0-9])?$");
    }
    p["overwritePolicy"]["default"] = json!("Never");
}
pub fn status(status: &mut Value) {
    let exports = &mut status["properties"]["exports"];
    exports.as_object_mut().unwrap().remove("default");
    exports["maxItems"] = json!(8);
    exports["x-kubernetes-list-type"] = json!("map");
    exports["x-kubernetes-list-map-keys"] = json!(["name"]);
    let p = &mut exports["items"]["properties"];
    for (field, limit) in [
        ("name", 63),
        ("configurationDigest", 71),
        ("releaseUid", 128),
        ("localDigest", 71),
        ("remoteDigest", 71),
        ("remoteReference", 384),
        ("phase", 32),
        ("attemptTime", 64),
        ("reason", 128),
    ] {
        p[field]["maxLength"] = json!(limit);
    }
    p["attempts"]["maximum"] = json!(32);
}

//! Structural API constraints for the deliberately narrow build declaration.
use anyhow::{Context, Result};
use serde_json::{Value, json};
fn member(union: &Value, key: &str) -> Result<Value> {
    union
        .get("properties")
        .and_then(|v| v.get(key))
        .cloned()
        .or_else(|| {
            union
                .get("oneOf")
                .and_then(Value::as_array)
                .and_then(|values| {
                    values
                        .iter()
                        .find_map(|v| v.get("properties").and_then(|v| v.get(key)).cloned())
                })
        })
        .with_context(|| format!("MissingBuildSchemaMember:{key}"))
}
fn forbidden() -> Value {
    json!({"type":"object","x-kubernetes-preserve-unknown-fields":true,"x-kubernetes-validations":[{"rule":"false","message":"Unsupported input source"}]})
}
fn bound_string(v: &mut Value, limit: usize) {
    v["maxLength"] = json!(limit);
}
fn file_mode(v: &mut Value) {
    v["minimum"] = json!(0);
    v["maximum"] = json!(511);
}
pub(crate) fn source(union: &Value) -> Result<Value> {
    let archive = member(union, "ociArchive")?;
    let mut build = member(union, "build")?;
    let props = &mut build["properties"];
    let image_inputs = &mut props["imageInputs"];
    image_inputs["maxItems"] = json!(8);
    image_inputs["x-kubernetes-list-type"] = json!("map");
    image_inputs["x-kubernetes-list-map-keys"] = json!(["name"]);
    let input = &mut image_inputs["items"]["properties"];
    bound_string(&mut input["name"], 63);
    input["name"]["pattern"] = json!("^[a-z]([a-z0-9-]{0,61}[a-z0-9])?$");
    bound_string(&mut input["image"], 512);
    input["image"]["pattern"] = json!(
        r"^platform://[a-z0-9.-]+/[a-z0-9.-]+(:=?|\?version==)[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$"
    );
    image_inputs["items"]["x-kubernetes-validations"] = json!([{"rule":"!(self.name in ['scratch', 'context', 'dockerfile'])","message":"Reserved image input name"}]);
    let definition = &mut props["definition"];
    *definition = json!({"type":"object","properties":{"inline":member(definition,"inline")?,"fromFile":member(definition,"fromFile")?},"x-kubernetes-validations":[{"rule":"has(self.inline) != has(self.fromFile)","message":"Select exactly one definition source"}]});
    bound_string(&mut definition["properties"]["inline"], 65536);
    definition["properties"]["inline"]["minLength"] = json!(1);
    bound_string(
        &mut definition["properties"]["fromFile"]["properties"]["path"],
        4096,
    );
    bound_string(&mut props["execution"]["properties"]["builderRef"], 63);
    props["execution"]["properties"]["builderRef"]["enum"] = json!(["default"]);
    props["cache"]["default"] = json!({"mode":"Local"});
    bound_string(&mut props["cache"]["properties"]["invalidationToken"], 128);
    props["buildSecrets"]["maxItems"] = json!(32);
    props["buildSecrets"]["x-kubernetes-list-type"] = json!("map");
    props["buildSecrets"]["x-kubernetes-list-map-keys"] = json!(["id"]);
    bound_string(&mut props["buildSecrets"]["items"]["properties"]["id"], 128);
    bound_string(
        &mut props["buildSecrets"]["items"]["properties"]["sourceFile"],
        4096,
    );
    let inputs = &mut props["inputs"];
    bound_string(&mut inputs["properties"]["contextPath"], 4096);
    for (field, key) in [("volumes", "name"), ("volumeMounts", "mountPath")] {
        let list = &mut inputs["properties"][field];
        list["maxItems"] = json!(32);
        list["x-kubernetes-list-type"] = json!("map");
        list["x-kubernetes-list-map-keys"] = json!([key]);
    }
    let volume = &mut inputs["properties"]["volumes"]["items"];
    volume["x-kubernetes-validations"] = json!([{"rule":"(has(self.configMap) ? 1 : 0) + (has(self.secret) ? 1 : 0) + (has(self.persistentVolumeClaim) ? 1 : 0) == 1","message":"Select exactly one supported volume source"}]);
    bound_string(&mut volume["properties"]["name"], 63);
    for field in [
        "hostPath",
        "emptyDir",
        "projected",
        "csi",
        "nfs",
        "downwardAPI",
    ] {
        volume["properties"][field] = forbidden();
    }
    for field in ["configMap", "secret"] {
        let object = &mut volume["properties"][field]["properties"];
        bound_string(&mut object["name"], 253);
        file_mode(&mut object["defaultMode"]);
        object["items"]["maxItems"] = json!(256);
        object["items"]["minItems"] = json!(1);
        object["items"]["x-kubernetes-list-type"] = json!("map");
        object["items"]["x-kubernetes-list-map-keys"] = json!(["path"]);
        let item = &mut object["items"]["items"]["properties"];
        bound_string(&mut item["key"], 253);
        bound_string(&mut item["path"], 4096);
        file_mode(&mut item["mode"]);
    }
    volume["properties"]["persistentVolumeClaim"]["required"] = json!(["claimName", "readOnly"]);
    let pvc = &mut volume["properties"]["persistentVolumeClaim"]["properties"];
    bound_string(&mut pvc["claimName"], 253);
    pvc["readOnly"]["enum"] = json!([true]);
    pvc["readOnly"].as_object_mut().unwrap().remove("default");
    inputs["properties"]["volumeMounts"]["items"]["required"] =
        json!(["name", "mountPath", "readOnly"]);
    let mount = &mut inputs["properties"]["volumeMounts"]["items"]["properties"];
    bound_string(&mut mount["name"], 63);
    bound_string(&mut mount["mountPath"], 4096);
    bound_string(&mut mount["subPath"], 4096);
    mount["readOnly"]["enum"] = json!([true]);
    mount["readOnly"].as_object_mut().unwrap().remove("default");
    for field in ["subPathExpr", "mountPropagation"] {
        mount[field] = json!({"type":"string","maxLength":4096,"x-kubernetes-validations":[{"rule":"false","message":"Unsupported mount control"}]});
    }
    Ok(
        json!({"type":"object","properties":{"ociArchive":archive,"build":build,"externalRegistry":forbidden()},"x-kubernetes-validations":[{"rule":"has(self.ociArchive) != has(self.build)","message":"Select exactly one supported source"}]}),
    )
}

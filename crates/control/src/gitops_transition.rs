//! Forward-compatible handoff to the installed newer release on Argo downgrade.
use crate::{component_images::ImageCatalog, gitops::Desired};
use anyhow::{Context, Result, ensure};
use k8s_openapi::api::{batch::v1::Job, core::v1::ConfigMap};
use kube::{
    Api, Client, ResourceExt,
    api::{DeleteParams, PostParams, Preconditions},
};
use serde_json::{Value, json};

pub(crate) fn configuration_identity(desired: &Desired) -> Result<String> {
    Ok(kimage_core::oci::digest(&serde_json::to_vec(desired)?))
}

fn job(
    desired: &Desired,
    installed: &ConfigMap,
    owner: &ConfigMap,
    source: &ImageCatalog,
) -> Result<Job> {
    source.validate(&source.release, false)?;
    let o = &desired.options;
    let uid = installed.uid().context("MissingInstallationUid")?;
    let configuration = configuration_identity(desired)?;
    let identity = kimage_core::oci::digest(&serde_json::to_vec(&(&uid, &configuration, source))?);
    let name = format!("{}-transition-{}", o.installation, &identity[7..23]);
    Ok(serde_json::from_value(json!({
        "apiVersion":"batch/v1", "kind":"Job",
        "metadata":{"name":name,"namespace":o.namespace,
            "labels":{"kimage.dev/gitops-transition":o.installation},
            "ownerReferences":[{"apiVersion":"v1","kind":"ConfigMap","name":owner.name_any(),"uid":owner.uid().context("MissingGitOpsOwnerUid")?,"controller":false,"blockOwnerDeletion":false}]},
        "spec":{"backoffLimit":6,"activeDeadlineSeconds":1200,"ttlSecondsAfterFinished":600,
            "template":{"metadata":{"labels":{"kimage.dev/gitops-transition":o.installation}},"spec":{
                "serviceAccountName":format!("{}-provisioner",o.installation),
                "restartPolicy":"Never","terminationGracePeriodSeconds":30,
                "securityContext":{"runAsNonRoot":true,"runAsUser":65532,"runAsGroup":65532,"seccompProfile":{"type":"RuntimeDefault"}},
                "containers":[{"name":"transition","image":source.reference("operator")?,"imagePullPolicy":"IfNotPresent",
                    "command":["/kimage-operator"],"args":["provision","--configuration","/desired/desired.json","--once"],
                    "env":[{"name":"KIMAGE_GITOPS_CONFIGURATION","value":configuration},{"name":"KIMAGE_GITOPS_INSTALLATION_UID","value":uid}],
                    "securityContext":{"allowPrivilegeEscalation":false,"readOnlyRootFilesystem":true,"capabilities":{"drop":["ALL"]}},
                    "resources":{"requests":{"cpu":"10m","memory":"32Mi"},"limits":{"memory":"256Mi"}},
                    "volumeMounts":[{"name":"desired","mountPath":"/desired","readOnly":true}]}],
                "volumes":[{"name":"desired","configMap":{"name":owner.name_any(),"defaultMode":420}}]
            }}}
    }))?)
}

// Permit API defaulted fields, while checking every declared privilege, image,
// identity, command and owner. A same-named foreign Job must never be adopted.
fn contains(actual: &Value, expected: &Value) -> bool {
    match (actual, expected) {
        (Value::Object(a), Value::Object(e)) => e
            .iter()
            .all(|(k, v)| a.get(k).is_some_and(|a| contains(a, v))),
        (Value::Array(a), Value::Array(e)) => {
            a.len() == e.len() && a.iter().zip(e).all(|(a, e)| contains(a, e))
        }
        _ => actual == expected,
    }
}

pub(crate) async fn delegate(
    client: Client,
    desired: &Desired,
    installed: &ConfigMap,
    source: &ImageCatalog,
) -> Result<()> {
    let o = &desired.options;
    let owner = Api::<ConfigMap>::namespaced(client.clone(), &o.namespace)
        .get(&format!("{}-provisioner", o.installation))
        .await?;
    ensure!(
        owner.labels().get("kimage.dev/provisioner") == Some(&o.installation)
            && owner.metadata.deletion_timestamp.is_none(),
        "GitOpsOwnerConflict"
    );
    let current: Desired = serde_json::from_str(
        owner
            .data
            .as_ref()
            .and_then(|d| d.get("desired.json"))
            .context("MissingGitOpsConfiguration")?,
    )?;
    ensure!(
        configuration_identity(&current)? == configuration_identity(desired)?,
        "GitOpsDesiredStateChanged"
    );
    let expected = job(desired, installed, &owner, source)?;
    let jobs: Api<Job> = Api::namespaced(client, &o.namespace);
    if let Some(existing) = jobs.get_opt(&expected.name_any()).await? {
        let actual = serde_json::to_value(&existing)?;
        let declared = serde_json::to_value(&expected)?;
        ensure!(
            contains(
                &actual["metadata"]["labels"],
                &declared["metadata"]["labels"]
            ) && actual["metadata"]["ownerReferences"] == declared["metadata"]["ownerReferences"]
                && contains(&actual["spec"], &declared["spec"]),
            "GitOpsTransitionJobConflict"
        );
        let finished = existing
            .status
            .as_ref()
            .and_then(|s| s.conditions.as_ref())
            .is_some_and(|c| {
                c.iter().any(|c| {
                    c.status == "True" && matches!(c.type_.as_str(), "Complete" | "Failed")
                })
            });
        if finished && existing.metadata.deletion_timestamp.is_none() {
            // Allows another round trip or recovery after exhausted retries.
            jobs.delete(
                &existing.name_any(),
                &DeleteParams {
                    preconditions: Some(Preconditions {
                        uid: existing.uid(),
                        resource_version: existing.resource_version(),
                    }),
                    ..Default::default()
                },
            )
            .await?;
        }
    } else {
        jobs.create(&PostParams::default(), &expected).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn declared_job_contract_allows_defaults_but_refuses_changed_execution() {
        let expected = json!({"containers":[{"name":"transition","image":"registry/operator@sha256:abc","args":["provision","--configuration","/desired/desired.json","--once"]}],"serviceAccountName":"installation-provisioner"});
        let mut actual = expected.clone();
        actual["dnsPolicy"] = json!("ClusterFirst");
        assert!(contains(&actual, &expected));
        actual["containers"][0]["image"] = json!("foreign");
        assert!(!contains(&actual, &expected));
        actual = expected.clone();
        actual["containers"][0]["args"] = json!(["provision"]);
        assert!(!contains(&actual, &expected));
        actual = expected.clone();
        actual["containers"]
            .as_array_mut()
            .unwrap()
            .push(json!({"name":"extra"}));
        assert!(!contains(&actual, &expected));
        actual = expected.clone();
        actual["serviceAccountName"] = json!("foreign");
        assert!(!contains(&actual, &expected));
    }
}

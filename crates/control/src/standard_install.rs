//! One-command installation using the existing guarded, resumable phases.
use crate::{
    bundle::Bundle,
    install::{Installer, LABEL},
};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::{future::Future, path::Path};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Bootstrap,
    Seed,
    Handover,
    ControlPlane,
    Builder,
}
const PHASES: [Phase; 5] = [
    Phase::Bootstrap,
    Phase::Seed,
    Phase::Handover,
    Phase::ControlPlane,
    Phase::Builder,
];

fn resume_phase(
    state: Option<&Value>,
    installation: &str,
    bundle: &str,
    configuration: &str,
) -> Result<Phase> {
    let Some(state) = state else {
        return Ok(Phase::Bootstrap);
    };
    ensure!(
        state["metadata"]["labels"][LABEL] == installation
            && state["data"]["bundle"] == bundle
            && state["data"]["configuration"] == configuration,
        "InstallationConfigurationConflict"
    );
    Ok(match state["data"]["phase"].as_str() {
        Some("Preparing") => Phase::Bootstrap,
        Some("CarrierServing") => Phase::Seed,
        // Handover also resumes its own replacement checkpoints and cleanup.
        Some(
            "ComponentsVerified" | "ReplacementsWarmed" | "OrdinaryRegistry" | "OrdinaryServing",
        ) => Phase::Handover,
        _ => anyhow::bail!(
            "UnsupportedInstallationCheckpoint: use the matching recovery/upgrade command"
        ),
    })
}

async fn run_from<F, Fut>(first: Phase, mut run: F) -> Result<()>
where
    F: FnMut(Phase) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    for phase in PHASES.into_iter().skip_while(|phase| *phase != first) {
        tracing::info!(?phase, "install phase");
        run(phase).await.with_context(|| {
            format!("InstallPhaseFailed: {phase:?}; rerun with identical arguments to resume")
        })?;
    }
    Ok(())
}

impl Installer {
    /// Fetching/verification happens in the CLI before this API-only workflow.
    pub async fn install(
        &self,
        bundle: &Bundle,
        admission_credentials: &Path,
        builder_credentials: &Path,
    ) -> Result<()> {
        ensure!(
            bundle.inventory.api_schema_revision == Some(crate::schema::REVISION),
            "BundleApiSchemaMismatch"
        );
        let app = self
            .options
            .application_namespace
            .as_deref()
            .context("ApplicationNamespaceRequired")?;
        ensure!(
            app != self.options.namespace
                && self.options.namespace != "default"
                && !self.options.namespace.starts_with("kube-"),
            "DedicatedSystemNamespaceRequired"
        );
        ensure!(
            bundle
                .inventory
                .components
                .get("builder")
                .is_some_and(|component| component.version == "0.33.0"),
            "UnsupportedDefaultBuilderVersion"
        );
        // Refuse missing or invalid material for later phases before bootstrap writes.
        crate::credentials::validate_registry_leaf(
            format!("{}.{}.svc", self.name("registry"), self.options.namespace),
            &self.options.credentials,
        )
        .await?;
        crate::credentials::validate_admission_leaf(
            format!("{}.{}.svc", self.name("admission"), self.options.namespace),
            admission_credentials,
        )
        .await?;
        crate::builder_install::credential_material(builder_credentials)?;
        let state = self
            .api_at("ConfigMap", Some(&self.options.namespace))?
            .get_opt(&self.name("state"))
            .await?;
        let state = state.map(serde_json::to_value).transpose()?;
        let first = resume_phase(
            state.as_ref(),
            &self.options.installation,
            &bundle.digest,
            &self.configuration(&bundle.digest)?,
        )?;
        run_from(first, |phase| async move {
            match phase {
                Phase::Bootstrap => self.bootstrap(bundle).await,
                Phase::Seed => self.seed(bundle).await,
                Phase::Handover => self.handover(bundle).await,
                Phase::ControlPlane => self.control_plane(bundle, admission_credentials).await,
                Phase::Builder => self.builder(bundle, builder_credentials).await,
            }
        })
        .await?;
        tracing::info!(installation = %self.options.installation, "installation ready");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::RefCell;

    fn state(phase: &str) -> Value {
        json!({"metadata":{"labels":{LABEL:"dev"}},"data":{"bundle":"sha256:bundle","configuration":"sha256:config","phase":phase}})
    }

    #[test]
    fn resume_never_rebootstraps_partial_or_completed_handover() {
        for (checkpoint, expected) in [
            ("Preparing", Phase::Bootstrap),
            ("CarrierServing", Phase::Seed),
            ("ComponentsVerified", Phase::Handover),
            ("ReplacementsWarmed", Phase::Handover),
            ("OrdinaryRegistry", Phase::Handover),
            ("OrdinaryServing", Phase::Handover),
        ] {
            assert_eq!(
                resume_phase(
                    Some(&state(checkpoint)),
                    "dev",
                    "sha256:bundle",
                    "sha256:config"
                )
                .unwrap(),
                expected
            );
        }
        assert_eq!(
            resume_phase(None, "dev", "sha256:bundle", "sha256:config").unwrap(),
            Phase::Bootstrap
        );
    }

    #[test]
    fn resume_refuses_foreign_changed_and_unknown_checkpoints() {
        for pointer in [
            "/metadata/labels/kimage.dev~1installation",
            "/data/bundle",
            "/data/configuration",
            "/data/phase",
        ] {
            let mut checkpoint = state("OrdinaryServing");
            *checkpoint.pointer_mut(pointer).unwrap() = json!("unexpected");
            assert!(
                resume_phase(Some(&checkpoint), "dev", "sha256:bundle", "sha256:config").is_err()
            );
        }
    }

    #[tokio::test]
    async fn install_stops_on_failure_and_resumes_without_repeating_bootstrap() {
        let calls = RefCell::new(Vec::new());
        let error = run_from(Phase::Bootstrap, |phase| {
            calls.borrow_mut().push(phase);
            async move {
                ensure!(phase != Phase::Handover, "interrupted");
                Ok(())
            }
        })
        .await
        .unwrap_err();
        assert!(error.to_string().contains("Handover"));
        assert_eq!(
            *calls.borrow(),
            [Phase::Bootstrap, Phase::Seed, Phase::Handover]
        );
        calls.borrow_mut().clear();
        let first = resume_phase(
            Some(&state("ReplacementsWarmed")),
            "dev",
            "sha256:bundle",
            "sha256:config",
        )
        .unwrap();
        run_from(first, |phase| {
            calls.borrow_mut().push(phase);
            async { Ok(()) }
        })
        .await
        .unwrap();
        assert_eq!(
            *calls.borrow(),
            [Phase::Handover, Phase::ControlPlane, Phase::Builder]
        );
    }
}

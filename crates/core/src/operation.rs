//! Durable state is supplied by the Kubernetes status CAS, never an in-memory task.
use crate::oci::{Descriptor, digest};
use anyhow::{Result, ensure};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Identity {
    pub image_uid: String,
    pub generation: i64,
    pub version: String,
    /// Ordinary frozen inputs/definition/platform; excludes execution/cache placement.
    pub snapshot: String,
}
impl Identity {
    pub fn id(&self) -> String {
        digest(&serde_json::to_vec(self).expect("identity serialization"))[7..].into()
    }
    pub fn current(&self, uid: &str, generation: i64) -> Result<()> {
        ensure!(
            self.image_uid == uid && self.generation == generation,
            "StaleOperation"
        );
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Phase {
    Capturing,
    Executing,
    Verified,
    Publishing,
    Published,
    Cancelled,
    Failed,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Operation {
    pub identity: Identity,
    pub attempt: u32,
    pub phase: Phase,
    pub root: Option<Descriptor>,
}
impl Operation {
    pub fn transition(&mut self, next: Phase, uid: &str, generation: i64) -> Result<()> {
        self.identity.current(uid, generation)?;
        ensure!(
            self.phase == next
                || matches!(
                    (self.phase, next),
                    (Phase::Capturing, Phase::Verified)
                        | (Phase::Capturing, Phase::Executing)
                        | (Phase::Executing, Phase::Verified)
                        | (Phase::Verified, Phase::Publishing)
                        | (Phase::Publishing, Phase::Published)
                        | (
                            Phase::Capturing
                                | Phase::Executing
                                | Phase::Verified
                                | Phase::Publishing,
                            Phase::Cancelled | Phase::Failed
                        )
                ),
            "InvalidOperationTransition"
        );
        if matches!(next, Phase::Verified | Phase::Publishing | Phase::Published) {
            ensure!(self.root.is_some(), "MissingVerifiedRoot");
        }
        self.phase = next;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restart_and_stale_completion() {
        let mut op = Operation {
            identity: Identity {
                image_uid: "uid".into(),
                generation: 1,
                version: "1.0.0".into(),
                snapshot: digest(b"snapshot"),
            },
            attempt: 1,
            phase: Phase::Capturing,
            root: None,
        };
        assert!(op.transition(Phase::Published, "uid", 1).is_err());
        op.root = Some(crate::oci::descriptor(b"manifest", crate::oci::MANIFEST));
        op.transition(Phase::Verified, "uid", 1).unwrap();
        let mut recovered: Operation =
            serde_json::from_slice(&serde_json::to_vec(&op).unwrap()).unwrap();
        assert!(recovered.transition(Phase::Publishing, "uid", 2).is_err());
        recovered.transition(Phase::Publishing, "uid", 1).unwrap();
        recovered.transition(Phase::Published, "uid", 1).unwrap();
        recovered.transition(Phase::Published, "uid", 1).unwrap();
        assert!(recovered.transition(Phase::Cancelled, "uid", 1).is_err());
    }
}

pub mod admission;
pub mod api;
pub mod api_compatibility;
pub mod bundle;
pub mod diagnostics;
pub mod discovery;
pub mod gateway;
pub mod gitops;
mod gitops_transition;
pub mod install;
pub mod kubernetes;
pub mod lifecycle;
pub mod metrics;
pub mod preflight;
pub mod proxy;
pub mod reconcile;
pub mod registry;
pub mod registry_check;
pub mod release_bundle;
pub mod retention;
pub mod schema;
pub mod serving;
pub mod tunnel;

pub mod component_images;
pub mod configuration;
pub mod control_install;
pub mod credentials;
pub mod pull_install;

pub mod build_inputs;
pub mod build_lifecycle;
pub mod build_logs;
pub mod build_reconcile;
mod build_schema;
pub mod build_session;
pub mod build_state;
pub mod builder_install;
pub mod containerfile;
pub mod direct;
pub mod direct_client;
pub mod handover;
pub mod image_inputs;
pub mod input_snapshot;

pub mod export_api;
pub mod export_install;
mod export_schema;
pub mod exporter;
pub mod node_install;
pub mod node_reconcile;
mod operation_lease;
pub mod upgrade;

mod standard_install;

mod platform_build;

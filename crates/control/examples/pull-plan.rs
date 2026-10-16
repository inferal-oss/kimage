//! Private manifest fixture exporter used by scripts/check-chart.py.
use anyhow::{Context, Result};
use kimage_control::{
    component_images::ImageCatalog,
    pull_install::{Options, Plan, credentials::Credentials},
};
use std::{io::Write, os::unix::fs::OpenOptionsExt, path::PathBuf};
fn main() -> Result<()> {
    let args = std::env::args()
        .skip(1)
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    anyhow::ensure!(
        args.len() == 4,
        "expected options, component catalog, nodes, and private output paths"
    );
    let options: Options = serde_json::from_slice(&std::fs::read(&args[0])?)?;
    let catalog: ImageCatalog = serde_json::from_slice(&std::fs::read(&args[1])?)?;
    let nodes: Vec<k8s_openapi::api::core::v1::Node> =
        serde_json::from_slice(&std::fs::read(&args[2])?)?;
    let credentials = Credentials::generate(&options.namespace, &options.installation)?;
    let plan = Plan::new(&options, &catalog, &nodes, &credentials)?;
    let mut resources = plan.resources;
    resources.extend(plan.webhooks);
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&args[3])
        .context("private fixture output must not already exist")?;
    output.write_all(&serde_json::to_vec(&resources)?)?;
    Ok(())
}

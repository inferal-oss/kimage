//! Explicitly enabled external-cluster acceptance. Public CI never needs cluster credentials.
#[test]
#[ignore = "requires a real mixed cluster and KIMAGE_MIXED_E2E_CONFIG; see docs/mixed-cluster-e2e.md"]
fn mixed_cluster_fresh_images_follow_workload_placement() {
    let config = std::env::var_os("KIMAGE_MIXED_E2E_CONFIG")
        .expect("set KIMAGE_MIXED_E2E_CONFIG to the non-secret JSON test configuration");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let result = std::process::Command::new("python3")
        .arg(root.join("scripts/e2e-mixed-cluster.py"))
        .args(["--run", "--config"])
        .arg(config)
        .status()
        .expect("start opt-in mixed-cluster E2E");
    assert!(result.success(), "mixed-cluster acceptance failed");
}

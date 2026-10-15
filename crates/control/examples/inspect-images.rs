//! Read-only decoder diagnostic; prints names and error classes, never inputs.
use serde_json::Value;
use std::io::Read;
fn main() -> anyhow::Result<()> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(16 * 1024 * 1024)
        .read_to_end(&mut bytes)?;
    let list: Value = serde_json::from_slice(&bytes)?;
    let mut failures = 0;
    for value in list["items"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("MissingItems"))?
    {
        if let Err(error) = serde_json::from_value::<kimage_control::api::Image>(value.clone()) {
            failures += 1;
            println!("{}: {:?}", value["metadata"]["name"], error.classify());
        }
    }
    println!("Image decode failures: {failures}");
    Ok(())
}

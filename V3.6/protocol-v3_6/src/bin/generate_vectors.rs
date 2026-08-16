use std::{env, fs, path::PathBuf};

use xhub_protocol_v3_6::generate_golden_vectors;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = env::args_os().nth(1).map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("test-vectors")
            .join("protocol-v3_6.json")
    });
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(&generate_golden_vectors()?)?;
    fs::write(&output, format!("{json}\n"))?;
    println!("wrote {}", output.display());
    Ok(())
}

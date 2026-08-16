use std::{fs, path::PathBuf};

use xhub_hub_v3_6::generate_hub_golden_vectors;

fn main() {
    let output = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("test-vectors")
        .join("hub-v3_6.json");
    fs::create_dir_all(output.parent().expect("vector parent")).expect("create vector directory");
    let vectors = generate_hub_golden_vectors().expect("generate HUB vectors");
    fs::write(
        &output,
        serde_json::to_string_pretty(&vectors).expect("serialize HUB vectors") + "\n",
    )
    .expect("write HUB vectors");
    println!("wrote {}", output.display());
}

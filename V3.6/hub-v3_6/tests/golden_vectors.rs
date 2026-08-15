use std::fs;

use serde_json::Value;
use xhub_hub_v3_6::generate_hub_golden_vectors;

#[test]
fn committed_hub_vectors_are_deterministic() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/test-vectors/hub-v3_6.json");
    let committed: Value =
        serde_json::from_str(&fs::read_to_string(path).expect("committed HUB vectors must exist"))
            .expect("committed HUB vectors must be valid JSON");
    assert_eq!(
        committed,
        generate_hub_golden_vectors().expect("generate HUB vectors")
    );
}

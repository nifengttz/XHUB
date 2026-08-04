#![no_main]

use clvmr::{Allocator, serde::node_from_bytes};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|bytes: &[u8]| {
    // Parsing is isolated from execution: production uses typed solutions.
    let mut allocator = Allocator::new();
    let _ = node_from_bytes(&mut allocator, bytes);
});

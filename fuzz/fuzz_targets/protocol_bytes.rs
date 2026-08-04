#![no_main]

use clvmr::{Allocator, serde::node_from_bytes};
use libfuzzer_sys::fuzz_target;
use wall_hub_mvp::{MerchantInvoice, PaymentIntent, PaymentVoucher};

fuzz_target!(|bytes: &[u8]| {
    let _ = MerchantInvoice::from_bytes(bytes);
    let _ = PaymentIntent::from_bytes(bytes);
    let _ = PaymentVoucher::from_bytes(bytes);

    let mut allocator = Allocator::new();
    let _ = node_from_bytes(&mut allocator, bytes);
});

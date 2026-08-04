use anyhow::Result;
use wall_hub_mvp::ChiaNode;

fn main() -> Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()?;
    let node = ChiaNode::testnet11()?;
    let status = runtime.block_on(node.status())?;
    println!("network={:?}", status.network_name);
    println!("synced={}", status.synced);
    println!("peak_height={}", status.peak_height);
    println!("peak_hash={}", status.peak_hash);
    println!("mempool_size={}", status.mempool_size);
    println!(
        "mempool_min_fee_per_5m_cost={}",
        status.mempool_min_fee_per_cost_unit
    );
    Ok(())
}

use std::{fs, path::PathBuf};

use chia_bls::SecretKey;
use rand::{RngCore, rngs::OsRng};
use serde_json::json;
use xhub_protocol_v3_6::public_key_bytes;

fn main() {
    if let Err(error) = run() {
        eprintln!("Identity generation failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let output = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or_else(|| "usage: generate-single-vps-identities OUTPUT_DIRECTORY".to_string())?;
    if output.exists() {
        return Err(format!(
            "output directory already exists: {}",
            output.display()
        ));
    }
    fs::create_dir_all(&output).map_err(|error| error.to_string())?;

    let mut attesters = Vec::with_capacity(3);
    for suffix in ['a', 'b', 'c'] {
        let mut seed = [0_u8; 32];
        OsRng.fill_bytes(&mut seed);
        let secret_key = SecretKey::from_seed(&seed);
        seed.fill(0);

        let signer_id = format!("wt-{suffix}");
        let secret_path = output.join(format!("{signer_id}-bls-secret-key.hex"));
        fs::write(
            &secret_path,
            format!("{}\n", hex::encode(secret_key.to_bytes())),
        )
        .map_err(|error| error.to_string())?;
        restrict_secret(&secret_path)?;

        attesters.push(json!({
            "signer_id": signer_id,
            "failure_domain": "single-tencent-vps",
            "signer_public_key": hex::encode(public_key_bytes(&secret_key)),
        }));
    }

    let public_path = output.join("custody-attesters.single-vps.local.json");
    let content = serde_json::to_string_pretty(&attesters).map_err(|error| error.to_string())?;
    fs::write(&public_path, format!("{content}\n")).map_err(|error| error.to_string())?;
    println!(
        "Generated three single-VPS test identities in {}",
        output.display()
    );
    Ok(())
}

#[cfg(unix)]
fn restrict_secret(path: &std::path::Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|error| error.to_string())
}

#[cfg(not(unix))]
fn restrict_secret(_path: &std::path::Path) -> Result<(), String> {
    Ok(())
}

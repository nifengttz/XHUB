fn main() {
    if let Err(error) = wall_hub_mvp::run_role_cli("merchant") {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

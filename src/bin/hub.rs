fn main() {
    if let Err(error) = wall_hub_mvp::run_role_cli("hub") {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

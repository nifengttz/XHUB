use std::env;

#[tokio::main]
async fn main() {
    let mut args = env::args().skip(1);
    let config = match (args.next().as_deref(), args.next()) {
        (Some("--config"), Some(path)) if args.next().is_none() => path,
        _ => {
            eprintln!("usage: hub-api --config <hub-api.json>");
            std::process::exit(2);
        }
    };
    if let Err(error) = wall_hub_mvp::run_hub_api(config).await {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

mod config;
mod download;
mod scan;
mod sde_version;
mod store;
mod tools;

use clap::Parser;
use config::Config;

fn main() {
    let cfg = Config::parse();
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&cfg.log_level)),
        )
        .init();

    let result = download::check_and_update(&cfg).unwrap_or_else(|e| {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    });

    if result.was_downloaded {
        eprintln!(
            "SDE build {} ({}) ready",
            result.build, result.release_date
        );
    } else {
        eprintln!("SDE build {} is current", result.build);
    }

    let start = std::time::Instant::now();
    let _store = scan::scan_sde(
        &cfg.sde_dir(result.build),
        result.build,
        &result.release_date,
    )
    .unwrap_or_else(|e| {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    });
    tracing::debug!("scan complete in {:.2}s", start.elapsed().as_secs_f64());
}

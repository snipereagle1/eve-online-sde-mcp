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
}

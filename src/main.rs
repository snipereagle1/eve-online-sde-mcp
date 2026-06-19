mod config;
mod download;
mod scan;
mod sde_version;
mod store;
mod tools;

use clap::Parser;
use config::Config;
use rmcp::ServiceExt;
use tools::SdeMcpServer;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg = Config::parse();
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&cfg.log_level)),
        )
        .init();

    let language = cfg.language.clone();

    let store = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let result = download::check_and_update(&cfg)?;

        if result.was_downloaded {
            eprintln!("SDE build {} ({}) ready", result.build, result.release_date);
        } else {
            eprintln!("SDE build {} is current", result.build);
        }

        let start = std::time::Instant::now();
        let store = scan::scan_sde(
            &cfg.sde_dir(result.build),
            result.build,
            &result.release_date,
        )?;
        tracing::debug!("scan complete in {:.2}s", start.elapsed().as_secs_f64());
        Ok(store)
    })
    .await
    .unwrap_or_else(|e| Err(anyhow::anyhow!("startup task panicked: {e}")))
    .unwrap_or_else(|e| {
        eprintln!("error: {e:#}");
        std::process::exit(1)
    });

    let server = SdeMcpServer::new(store, language);
    let transport = rmcp::transport::io::stdio();
    server.serve(transport).await?.waiting().await?;
    Ok(())
}

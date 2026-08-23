use clap::Parser;
use reddit_rss_mcp::mcp;
use tracing::info;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

/// Read Reddit via public Atom/RSS only, exposed as an MCP server.
#[derive(Parser, Debug)]
#[command(name = "reddit-rss-mcp", version, about)]
struct Cli {
    /// Use Streamable HTTP transport (default: stdio).
    #[arg(long)]
    http: bool,

    /// Address to bind for --http (default 127.0.0.1:8085).
    #[arg(long, default_value = "127.0.0.1:8085")]
    bind: String,

    /// Use stdio transport (default; accepted for symmetry with --http).
    #[arg(long)]
    stdio: bool,
}

fn init_logging() {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("reddit_rss_mcp=info,warn"));
    let json = std::env::var("REDDIT_RSS_LOG_JSON")
        .map(|v| v == "1")
        .unwrap_or(false);

    if json {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt::layer().json().with_writer(std::io::stderr))
            .init();
    } else {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt::layer().with_writer(std::io::stderr).with_target(true))
            .init();
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_logging();
    let cli = Cli::parse();
    info!("reddit-rss-mcp starting");

    if cli.http {
        let key = std::env::var("REDDIT_RSS_MCP_KEY").ok();
        mcp::run_http(&cli.bind, &key).await?;
    } else {
        // --stdio is the default; ignore if explicitly passed.
        let _ = cli.stdio;
        mcp::run_stdio().await?;
    }
    Ok(())
}

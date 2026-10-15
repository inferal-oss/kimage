use clap::Parser;
use std::{net::SocketAddr, path::PathBuf};
#[derive(Parser)]
#[command(version = kimage_control::serving::VERSION)]
struct Args {
    #[arg(long, default_value = "[::]:5443")]
    listen: SocketAddr,
    #[arg(long)]
    cert: PathBuf,
    #[arg(long)]
    key: PathBuf,
    #[arg(long)]
    read_password: PathBuf,
    #[arg(long)]
    write_password: PathBuf,
    #[arg(long, value_delimiter = ',')]
    namespaces: Vec<String>,
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().json().init();
    let a = Args::parse();
    anyhow::ensure!(!a.namespaces.is_empty(), "NamespaceAllowlistRequired");
    let r = kimage_control::gateway::router(
        a.namespaces.into_iter().collect(),
        a.read_password,
        a.write_password,
    )?;
    kimage_control::serving::tls(a.listen, a.cert, a.key, r).await
}

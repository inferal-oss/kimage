use clap::Parser;
use std::{net::SocketAddr, path::PathBuf};
#[derive(Parser)]
#[command(name = "kimage-proxy", version = kimage_control::serving::VERSION)]
struct Args {
    #[arg(long)]
    listen: SocketAddr,
    #[arg(long)]
    upstream: reqwest::Url,
    #[arg(long)]
    ca: PathBuf,
    #[arg(long)]
    password_file: PathBuf,
    #[arg(long, value_delimiter = ',')]
    namespaces: Vec<String>,
    #[arg(long)]
    overlap: bool,
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().json().init();
    let a = Args::parse();
    kimage_control::proxy::Proxy::new(
        a.upstream,
        &std::fs::read(a.ca)?,
        a.namespaces.into_iter().collect(),
        a.password_file,
    )?
    .serve(a.listen, a.overlap)
    .await
}

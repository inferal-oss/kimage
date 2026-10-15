use clap::Parser;
use std::path::PathBuf;
#[derive(Parser)]
#[command(
    name = "kimage-admission",
    version,
    about = "Resolve kimage image selectors to verified digest-pinned Pod images"
)]
struct Args {
    #[arg(long)]
    builder_image: Option<String>,
    #[arg(long)]
    namespace: String,
    #[arg(long)]
    endpoint: String,
    #[arg(long, value_delimiter = ',')]
    nodes: Vec<String>,
    #[arg(long)]
    cert: PathBuf,
    #[arg(long)]
    key: PathBuf,
    #[arg(long)]
    resolution_key: PathBuf,
    #[arg(long, requires = "input_image")]
    operator_identity: Option<String>,
    #[arg(long, requires = "operator_identity")]
    input_image: Option<String>,
    #[arg(long, requires = "installation")]
    system_namespace: Option<String>,
    #[arg(long, requires = "system_namespace")]
    installation: Option<String>,
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().json().init();
    let a = Args::parse();
    kimage_control::admission::serve(
        kimage_control::admission::Admission {
            client: kimage_control::kubernetes::client().await?,
            namespace: a.namespace,
            endpoint: a.endpoint,
            nodes: a.nodes.into_iter().collect(),
            key_file: a.resolution_key,
            input_identity: a.operator_identity.zip(a.input_image),
            builder_image: a.builder_image,
            readiness_scope: a.system_namespace.zip(a.installation),
        },
        a.cert,
        a.key,
    )
    .await
}

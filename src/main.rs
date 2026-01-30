use clap::Parser;
use httpet::config::setup_logging;
use tracing::error;

#[tokio::main(flavor = "multi_thread", worker_threads = 32)]
async fn main() {
    let cli = httpet::cli::CliOptions::parse();

    setup_logging(cli.debug);

    if let Err(err) = httpet::web::setup_server(&cli.listen_address, cli.port).await {
        error!("Application error: {}", err);
    }
}

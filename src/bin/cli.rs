#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    if let Err(err) = swap_orchestrator::cli::run().await {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}

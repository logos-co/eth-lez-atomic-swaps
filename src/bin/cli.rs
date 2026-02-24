#[tokio::main]
async fn main() {
    if let Err(err) = swap_orchestrator::cli::run().await {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}

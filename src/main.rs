use anyhow::Result;
use mazit::engine::{Core, data_directory};

fn main() {
    let logging = match mazit::logging::init() {
        Ok(logging) => logging,
        Err(error) => {
            eprintln!(
                "{}",
                mazit::redact(&format!("Initialize application logs: {error:#}"))
            );
            std::process::exit(1);
        }
    };
    if let Err(error) = run() {
        log::error!("Application failed: {error:#}");
        logging.shutdown();
        std::process::exit(1);
    }
    log::info!("Mazit stopped");
    logging.shutdown();
}
fn run() -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let core = Core::new(data_directory()?, None)?;
    let engine = mazit::engine::Engine::start(core, runtime.handle())?;
    mazit::desktop::run(engine);
    Ok(())
}

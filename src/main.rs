use anyhow::Result;
use mazit::engine::{Core, data_directory};

fn main() {
    if let Err(error) = run() {
        eprintln!("{}", mazit::redact(&format!("{error:#}")));
        std::process::exit(1);
    }
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

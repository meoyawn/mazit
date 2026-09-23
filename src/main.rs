use anyhow::Result;
use clap::{Parser, Subcommand};
use mazit::{
    config,
    engine::{Core, data_directory},
    storage::Storage,
};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    version,
    about = "YouTube playlists and channels, published as podcasts in your storage"
)]
struct Args {
    #[arg(long)]
    data_dir: Option<PathBuf>,
    /// Read a Netscape cookie jar. The original file is never modified.
    #[arg(long)]
    cookies: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Action>,
}
#[derive(Subcommand)]
enum Action {
    /// Synchronize once using config.toml (or another TOML file with --config).
    Sync {
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long)]
        source: Option<String>,
    },
    /// Inspect YouTube metadata without downloading media or publishing a feed.
    Inspect { url: String },
    /// Prepare a local audio file as fast-start M4A using the bundled FFmpeg libraries.
    Audio { input: PathBuf, output: PathBuf },
    /// List saved subscriptions.
    List,
}
fn main() {
    if let Err(error) = run() {
        eprintln!("{}", mazit::redact(&format!("{error:#}")));
        std::process::exit(1);
    }
}
fn run() -> Result<()> {
    let args = Args::parse();
    if let Some(Action::Audio { input, output }) = &args.command {
        println!(
            "{} bytes; fast-start M4A; FFmpeg {}",
            mazit::audio::prepare_m4a(input, output)?,
            mazit::audio::version()
        );
        return Ok(());
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let core = Core::new(
        args.data_dir.unwrap_or(data_directory()?),
        args.cookies.as_deref(),
    )?;
    match args.command {
        Some(Action::Inspect { url }) => runtime.block_on(async {
            let (kind, id, _) = core.youtube.resolve(&url).await?;
            let snapshot = core.youtube.snapshot(&kind, &id).await?;
            println!("{}: {} episodes", snapshot.title, snapshot.videos.len());
            if let Some(video) = snapshot.videos.iter().find(|v| v.available) {
                let media = core.youtube.media(&video.id).await?;
                println!("Audio available: {} bytes", media.bytes);
            }
            Ok(())
        }),
        Some(Action::Sync { config, source }) => runtime.block_on(async {
            let config_path = match config {
                Some(path) => path,
                None => config::path()?,
            };
            let config = config::load(&config_path)?;
            let storage = Storage::new(config.clone())?;
            storage.verify().await?;
            core.db.bind_storage(&config.identity())?;
            let ids = if let Some(url) = source {
                vec![core.add(&url).await?]
            } else {
                core.db.sources()?.into_iter().map(|s| s.id).collect()
            };
            for id in ids {
                core.sync(&id, &storage, &|| {}).await?;
                let source = core.db.source(&id)?;
                println!(
                    "{}: {}/{} uploaded; {}",
                    source.title, source.uploaded, source.total, source.phase
                );
            }
            Ok(())
        }),
        Some(Action::List) => {
            println!("{}", serde_json::to_string_pretty(&core.db.sources()?)?);
            Ok(())
        }
        Some(Action::Audio { .. }) => unreachable!(),
        None => {
            #[cfg(feature = "desktop")]
            {
                let engine = mazit::engine::Engine::start(core, runtime.handle())?;
                mazit::desktop::run(engine);
                Ok(())
            }
            #[cfg(not(feature = "desktop"))]
            {
                anyhow::bail!("Rebuild with the desktop feature to open the GPUI app")
            }
        }
    }
}

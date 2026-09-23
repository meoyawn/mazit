use anyhow::{Context, Result};
use flexi_logger::{Cleanup, Criterion, DeferredNow, Duplicate, FileSpec, Logger, LoggerHandle, Naming, WriteMode};
use std::{io::Write, path::{Path, PathBuf}};

const MAX_BYTES: u64 = 5 * 1024 * 1024;
const RETAINED_FILES: usize = 4;
const CURRENT_FILE: &str = "mazit_rCURRENT.log";

pub fn path() -> Result<PathBuf> {
    Ok(crate::engine::data_directory()?.join("logs").join(CURRENT_FILE))
}

fn logger(directory: &Path, max_bytes: u64) -> Result<Logger> {
    std::fs::create_dir_all(directory).context("Create log directory")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
    }
    // Keep SDK request diagnostics disabled: they can contain credentials and signed URLs.
    Ok(Logger::try_with_str("off,mazit=info,gpui=warn,gpui_component=warn")?
        .log_to_file(FileSpec::default().directory(directory).basename("mazit").suppress_timestamp())
        .append()
        .rotate(Criterion::Size(max_bytes), Naming::Numbers, Cleanup::KeepLogFiles(RETAINED_FILES))
        .cleanup_in_background_thread(false)
        .write_mode(WriteMode::Direct)
        .format(format)
        .duplicate_to_stderr(Duplicate::Warn))
}

pub fn init() -> Result<LoggerHandle> {
    let path = path()?;
    let handle = logger(path.parent().context("Log path has no directory")?, MAX_BYTES)?
        .start().context("Start file logging")?;
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic| {
        log::error!("Application panic: {panic}");
        log::logger().flush();
        previous_hook(panic);
    }));
    log::info!("Mazit {} started; pid={}", env!("CARGO_PKG_VERSION"), std::process::id());
    Ok(handle)
}

fn format(writer: &mut dyn Write, now: &mut DeferredNow, record: &log::Record<'_>) -> std::io::Result<()> {
    write!(writer, "{} {} [{}] {}", now.now().to_rfc3339(), record.level(), record.target(), crate::redact(&record.args().to_string()))
}

#[cfg(feature = "desktop")]
pub fn open_in_editor() -> Result<()> {
    log::info!("Opening application log in editor");
    log::logger().flush();
    let path = path()?;
    anyhow::ensure!(path.is_file(), "Application log file is missing");
    crate::editor::open(&path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logs_are_immediate_redacted_and_appended_across_restarts() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(CURRENT_FILE);
        for _ in 0..2 {
            let (logger, handle) = logger(directory.path(), MAX_BYTES).unwrap().build().unwrap();
            logger.log(&log::Record::builder().target("mazit::engine").level(log::Level::Info)
                .args(format_args!("Completed https://example.com/audio?token=secret\nnext line")).build());
            // Direct writes must be readable before shutdown or an explicit flush.
            let text = std::fs::read_to_string(&path).unwrap();
            assert!(text.contains("INFO [mazit::engine] Completed [URL] next line"));
            assert!(!text.contains("secret"));
            handle.shutdown();
        }
        assert_eq!(std::fs::read_to_string(path).unwrap().lines().count(), 2);
    }

    #[test]
    fn rotation_keeps_current_file_and_only_four_archives() {
        let directory = tempfile::tempdir().unwrap();
        let (logger, handle) = logger(directory.path(), 100).unwrap().build().unwrap();
        for entry in 0..30 {
            logger.log(&log::Record::builder().target("mazit::engine").level(log::Level::Info)
                .args(format_args!("Sync completed entry={entry:02}")).build());
        }
        handle.shutdown();
        let files: Vec<_> = std::fs::read_dir(directory.path()).unwrap().map(|e| e.unwrap().path()).collect();
        assert_eq!(files.len(), RETAINED_FILES + 1);
        let current = std::fs::read_to_string(directory.path().join(CURRENT_FILE)).unwrap();
        assert!(current.contains("entry=29"));
        for file in files {
            assert!(!std::fs::read_to_string(file).unwrap().contains("entry=00"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(directory.path()).unwrap().permissions().mode() & 0o777, 0o700);
        }
    }
}

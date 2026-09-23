use anyhow::{Context, Result, bail};
use std::path::Path;

pub fn open(path: &Path) -> Result<()> {
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "open -e".to_string());
    let parts = shlex::split(&editor).context("EDITOR has invalid quoting")?;
    let Some(program) = parts.first() else {
        bail!("EDITOR is empty");
    };
    let mut child = std::process::Command::new(program)
        .args(&parts[1..])
        .arg(path)
        .spawn()
        .with_context(|| format!("Launch editor {program}"))?;
    std::thread::spawn(move || match child.wait() {
        Ok(status) if status.success() => (),
        Ok(status) => log::warn!("Editor exited with {status}"),
        Err(error) => log::warn!("Wait for editor: {error}"),
    });
    Ok(())
}

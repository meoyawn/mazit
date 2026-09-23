use crate::storage::StorageConfig;
use anyhow::{Context, Result, anyhow};
use serde::Deserialize;
use std::{
    fs,
    path::{Path, PathBuf},
};

const TEMPLATE: &str = r#"# Fill in your S3-compatible storage details, then save this file.
[s3]
endpoint = ""
region = "auto"
bucket = ""
root = ""
public_base_url = ""
access_key_id = ""
secret_access_key = ""
"#;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    s3: S3Config,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct S3Config {
    endpoint: String,
    region: String,
    bucket: String,
    #[serde(default)]
    root: String,
    public_base_url: String,
    access_key_id: String,
    secret_access_key: String,
}

pub fn path() -> Result<PathBuf> {
    Ok(directories::BaseDirs::new()
        .context("Find home directory")?
        .home_dir()
        .join(".config/mazit/config.toml"))
}

pub fn read_text(path: &Path) -> Result<Option<String>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("Read {}", path.display())),
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let permissions = fs::metadata(path)?.permissions();
        if permissions.mode() & 0o077 != 0 {
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))
                .with_context(|| format!("Protect {}", path.display()))?;
        }
    }
    Ok(Some(text))
}

pub fn parse(text: &str) -> Result<StorageConfig> {
    let config: FileConfig = toml::from_str(text).map_err(|error: toml::de::Error| {
        let line = error.span().map(|span| {
            text.get(..span.start)
                .unwrap_or("")
                .bytes()
                .filter(|byte| *byte == b'\n')
                .count()
                + 1
        });
        match line {
            Some(line) => {
                anyhow!("Invalid config.toml at line {line}; check the [s3] keys and values")
            }
            None => anyhow!("Invalid config.toml; check the [s3] keys and values"),
        }
    })?;
    let S3Config {
        endpoint,
        region,
        bucket,
        root,
        public_base_url,
        access_key_id,
        secret_access_key,
    } = config.s3;
    Ok(StorageConfig::S3 {
        endpoint,
        region,
        bucket,
        root,
        public_base_url,
        access_key_id,
        secret_access_key,
    })
}

pub fn load(path: &Path) -> Result<StorageConfig> {
    let text = read_text(path)?.with_context(|| format!("No config file at {}", path.display()))?;
    parse(&text)
}

pub fn ensure_template(path: &Path) -> Result<()> {
    if !path.exists() {
        match write_new(path, TEMPLATE) {
            Ok(()) => (),
            Err(_error) if path.exists() => return Ok(()),
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn write_new(path: &Path, contents: &str) -> Result<()> {
    let parent = path
        .parent()
        .context("Config path has no parent directory")?;
    fs::create_dir_all(parent).with_context(|| format!("Create {}", parent.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    use std::io::Write;
    temporary.write_all(contents.as_bytes())?;
    temporary.as_file().sync_all()?;
    temporary.persist_noclobber(path)?;
    Ok(())
}

#[cfg(feature = "desktop")]
pub fn open_in_editor() -> Result<()> {
    let path = path()?;
    ensure_template(&path)?;
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "open -e".to_string());
    let parts = shlex::split(&editor).context("EDITOR has invalid quoting")?;
    let Some(program) = parts.first() else {
        anyhow::bail!("EDITOR is empty");
    };
    let mut child = std::process::Command::new(program)
        .args(&parts[1..])
        .arg(path)
        .spawn()
        .with_context(|| format!("Launch editor {program}"))?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = "[s3]\nendpoint = 'https://account.r2.cloudflarestorage.com'\nregion = 'auto'\nbucket = 'podcasts'\npublic_base_url = 'https://audio.example.com'\naccess_key_id = 'id'\nsecret_access_key = 'secret'\n";

    #[test]
    fn parses_s3_with_optional_empty_root() {
        let StorageConfig::S3 { root, region, .. } = parse(EXAMPLE).unwrap();
        assert_eq!(root, "");
        assert_eq!(region, "auto");
    }

    #[test]
    fn rejects_unknown_keys_without_exposing_secrets() {
        let text = EXAMPLE.replace(
            "secret_access_key = 'secret'",
            "secret_access_key = 'secret'\nsecret = 'sensitive'",
        );
        let error = parse(&text).err().unwrap().to_string();
        assert!(error.contains("Invalid config.toml"));
        assert!(!error.contains("sensitive"));
    }

    #[test]
    fn template_is_private_and_is_never_overwritten() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("mazit/config.toml");
        ensure_template(&path).unwrap();
        let template = fs::read_to_string(&path).unwrap();
        assert!(template.contains("[s3]"));
        fs::write(&path, EXAMPLE).unwrap();
        ensure_template(&path).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), EXAMPLE);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}

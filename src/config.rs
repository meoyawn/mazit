use crate::storage::StorageConfig;
use anyhow::{Context, Result, anyhow, ensure};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
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

pub fn bind_storage(directory: &Path, identity: &str, has_sources: bool) -> Result<()> {
    let path = directory.join("storage-binding");
    let existing = read_text(&path)?;
    ensure!(
        !has_sources || existing.as_deref() == Some(identity),
        "Existing feeds are tied to their storage location; restore the library's storage binding or move its feeds explicitly"
    );
    if existing.as_deref() != Some(identity) {
        use std::io::Write;
        fs::create_dir_all(directory)?;
        let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
        temporary.write_all(identity.as_bytes())?;
        temporary.as_file().sync_all()?;
        temporary.persist(path).context("Save storage binding")?;
    }
    Ok(())
}

pub fn export_legacy_storage_binding(directory: &Path) -> Result<()> {
    let database = directory.join("mazit.sqlite");
    if !database.exists() {
        return Ok(());
    }
    let connection = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    let has_settings: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type IS 'table' AND name IS 'settings')",
        [],
        |row| row.get(0),
    )?;
    if has_settings {
        let identity: Option<String> = connection
            .query_row(
                "SELECT value FROM settings WHERE key IS 'storage'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(identity) = identity {
            let path = directory.join("storage-binding");
            if let Some(existing) = read_text(&path)? {
                ensure!(
                    existing == identity,
                    "Storage binding conflicts with the library checkpoint"
                );
            } else {
                // Persist the binding before V2 removes its old SQL table, including across crashes.
                write_new(&path, &identity)?;
            }
        }
    }
    Ok(())
}

#[cfg(feature = "desktop")]
pub fn open_in_editor() -> Result<()> {
    let path = path()?;
    ensure_template(&path)?;
    crate::editor::open(&path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = "[s3]\nendpoint = 'https://account.r2.cloudflarestorage.com'\nregion = 'auto'\nbucket = 'podcasts'\npublic_base_url = 'https://audio.example.com'\naccess_key_id = 'id'\nsecret_access_key = 'secret'\n";

    #[test]
    fn storage_binding_is_a_file_and_cannot_repoint_existing_checkpoints() {
        let directory = tempfile::tempdir().unwrap();
        bind_storage(directory.path(), "first-location", false).unwrap();
        bind_storage(directory.path(), "first-location", true).unwrap();
        assert!(bind_storage(directory.path(), "other-location", true).is_err());
        assert_eq!(
            fs::read_to_string(directory.path().join("storage-binding")).unwrap(),
            "first-location"
        );
        bind_storage(directory.path(), "other-location", false).unwrap();
        assert!(bind_storage(directory.path(), "other-location", true).is_ok());
    }

    #[test]
    fn legacy_binding_is_exported_before_settings_are_removed() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("mazit.sqlite");
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch(include_str!("../migrations/V1__initial.sql"))
            .unwrap();
        connection.execute_batch("PRAGMA user_version=1; INSERT INTO settings VALUES('storage','original-location');").unwrap();
        export_legacy_storage_binding(directory.path()).unwrap();
        // Repeating the export after a crash is harmless.
        export_legacy_storage_binding(directory.path()).unwrap();
        drop(crate::database::Database::open(&database).unwrap());
        export_legacy_storage_binding(directory.path()).unwrap();
        bind_storage(directory.path(), "original-location", true).unwrap();
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name IS 'settings')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!exists);
    }

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

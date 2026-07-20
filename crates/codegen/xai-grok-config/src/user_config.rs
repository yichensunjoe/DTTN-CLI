//! Focused read/write access to the user-owned DTTN config layer.
//!
//! This module never reads managed configuration and never starts network or
//! model runtime work. Updates preserve unrelated TOML formatting and comments.

use std::path::{Path, PathBuf};
use std::str::FromStr;

use thiserror::Error;
use toml_edit::{DocumentMut, Item, Table, value};

#[derive(Debug, Error)]
pub enum UserConfigError {
    #[error("invalid model id: model must not be empty or contain control characters")]
    InvalidModelId,
    #[error("failed to read DTTN config at {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse DTTN config at {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml_edit::TomlError,
    },
    #[error("[models] in {path} is not a TOML table")]
    ModelsNotTable { path: PathBuf },
    #[error("failed to write DTTN config at {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub fn user_config_path() -> PathBuf {
    crate::dttn_home().join("config.toml")
}

pub fn user_default_model() -> Result<Option<String>, UserConfigError> {
    user_default_model_at(&user_config_path())
}

pub fn set_user_default_model(model: &str) -> Result<PathBuf, UserConfigError> {
    let path = user_config_path();
    set_user_default_model_at(&path, model)?;
    Ok(path)
}

pub fn reset_user_default_model() -> Result<bool, UserConfigError> {
    reset_user_default_model_at(&user_config_path())
}

fn validate_model_id(model: &str) -> Result<&str, UserConfigError> {
    let model = model.trim();
    if model.is_empty() || model.chars().any(char::is_control) {
        return Err(UserConfigError::InvalidModelId);
    }
    Ok(model)
}

fn load_document(path: &Path) -> Result<DocumentMut, UserConfigError> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(DocumentMut::new());
        }
        Err(source) => {
            return Err(UserConfigError::Read {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    DocumentMut::from_str(&raw).map_err(|source| UserConfigError::Parse {
        path: path.to_path_buf(),
        source,
    })
}

fn user_default_model_at(path: &Path) -> Result<Option<String>, UserConfigError> {
    let document = load_document(path)?;
    Ok(document
        .get("models")
        .and_then(Item::as_table_like)
        .and_then(|models| models.get("default"))
        .and_then(Item::as_value)
        .and_then(|value| value.as_str())
        .map(str::to_owned))
}

fn set_user_default_model_at(path: &Path, model: &str) -> Result<(), UserConfigError> {
    let model = validate_model_id(model)?;
    let mut document = load_document(path)?;
    if document.get("models").is_none() {
        document["models"] = Item::Table(Table::new());
    }
    let Some(models) = document.get_mut("models").and_then(Item::as_table_like_mut) else {
        return Err(UserConfigError::ModelsNotTable {
            path: path.to_path_buf(),
        });
    };
    models.insert("default", value(model));
    write_document(path, &document)
}

fn reset_user_default_model_at(path: &Path) -> Result<bool, UserConfigError> {
    if !path.exists() {
        return Ok(false);
    }
    let mut document = load_document(path)?;
    let removed = match document.get_mut("models") {
        Some(item) => item
            .as_table_like_mut()
            .ok_or_else(|| UserConfigError::ModelsNotTable {
                path: path.to_path_buf(),
            })?
            .remove("default")
            .is_some(),
        None => false,
    };
    if removed {
        write_document(path, &document)?;
    }
    Ok(removed)
}

fn write_document(path: &Path, document: &DocumentMut) -> Result<(), UserConfigError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| UserConfigError::Write {
            path: path.to_path_buf(),
            source,
        })?;
    }
    super::fs_atomic::write_atomically(path, &document.to_string(), Some(0o600)).map_err(|source| {
        UserConfigError::Write {
            path: path.to_path_buf(),
            source,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setting_model_preserves_comments_and_unrelated_tables() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        std::fs::write(
            &path,
            "# keep this comment\n[ui]\nscreen_mode = \"minimal\"\n",
        )
        .unwrap();
        set_user_default_model_at(&path, "anthropic/claude-sonnet").unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("# keep this comment"));
        assert!(raw.contains("screen_mode = \"minimal\""));
        assert!(raw.contains("default = \"anthropic/claude-sonnet\""));
    }

    #[test]
    fn reset_removes_only_the_default_model() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        std::fs::write(
            &path,
            "[models]\ndefault = \"old\"\nweb_search = \"search\"\n",
        )
        .unwrap();
        assert!(reset_user_default_model_at(&path).unwrap());
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("default ="));
        assert!(raw.contains("web_search = \"search\""));
    }

    #[test]
    fn invalid_existing_toml_is_never_overwritten() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        std::fs::write(&path, "[models\n").unwrap();
        assert!(matches!(
            set_user_default_model_at(&path, "model"),
            Err(UserConfigError::Parse { .. })
        ));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "[models\n");
    }
}

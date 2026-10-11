//! Per-job settings that later calls read back: the config file.

use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

/// Keys are spelled as `config --set` takes them.
#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Absolute path of the Template file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<PathBuf>,
    /// Overrides the default Comment Key.
    #[serde(
        default,
        rename = "comment-key",
        skip_serializing_if = "Option::is_none"
    )]
    pub comment_key: Option<String>,
}

/// A `config --set` argument: `KEY=VALUE`.
#[derive(Debug, Clone)]
pub struct Setting {
    key: String,
    value: String,
}

impl FromStr for Setting {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        let (key, value) = s
            .split_once('=')
            .ok_or_else(|| anyhow!("invalid setting {s:?}: expected KEY=VALUE"))?;
        Ok(Setting {
            key: key.trim().to_string(),
            value: value.trim().to_string(),
        })
    }
}

impl Config {
    /// A missing file is an empty Config.
    pub fn load(path: &Path) -> Result<Config> {
        match fs::read_to_string(path) {
            Ok(json) => serde_json::from_str(&json)
                .with_context(|| format!("config {} is not valid JSON", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(e).with_context(|| format!("reading config {}", path.display())),
        }
    }

    /// Reads a config supplied by `config --load`. Relative Template paths are
    /// resolved against the file's directory, and the Template is checked.
    pub fn from_file(path: &Path) -> Result<Config> {
        let json = fs::read_to_string(path)
            .with_context(|| format!("reading config {}", path.display()))?;
        let mut config: Config = serde_json::from_str(&json)
            .with_context(|| format!("config {} is not valid JSON", path.display()))?;
        if let Some(template) = &config.template {
            let base = path.parent().unwrap_or(Path::new("."));
            config.template = Some(check_template(&base.join(template))?);
        }
        Ok(config)
    }

    /// Applies one `config --set`. A relative Template path resolves against
    /// the current directory.
    pub fn set(&mut self, setting: &Setting) -> Result<()> {
        if setting.value.is_empty() {
            bail!("`{}` needs a value", setting.key);
        }
        match setting.key.as_str() {
            "template" => self.template = Some(check_template(Path::new(&setting.value))?),
            "comment-key" => self.comment_key = Some(setting.value.clone()),
            other => bail!("unknown config key {other:?}; known keys: template, comment-key"),
        }
        Ok(())
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        fs::write(path, json + "\n").with_context(|| format!("writing config {}", path.display()))
    }
}

/// The Template's absolute path, once it's known to exist and parse.
pub fn check_template(path: &Path) -> Result<PathBuf> {
    let path =
        fs::canonicalize(path).with_context(|| format!("reading Template {}", path.display()))?;
    let source = fs::read_to_string(&path)
        .with_context(|| format!("reading Template {}", path.display()))?;
    minijinja::Environment::new()
        .template_from_str(&source)
        .with_context(|| format!("Template {} is invalid", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_is_empty_and_saved_config_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/config.json");
        assert_eq!(Config::load(&path).unwrap(), Config::default());

        let config = Config {
            template: Some("/repo/t.md.j2".into()),
            comment_key: Some("prod".into()),
        };
        config.save(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), config);
    }

    #[test]
    fn from_file_resolves_the_template_against_the_config_dir() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("ci")).unwrap();
        fs::write(dir.path().join("ci/t.md.j2"), "ok").unwrap();
        let path = dir.path().join("ci/config.json");
        fs::write(&path, r#"{"template": "t.md.j2"}"#).unwrap();
        let config = Config::from_file(&path).unwrap();
        assert_eq!(
            config.template,
            Some(fs::canonicalize(dir.path().join("ci/t.md.j2")).unwrap())
        );
    }

    #[test]
    fn from_file_rejects_a_broken_template() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("t.md.j2"), "{% for %}").unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, r#"{"template": "t.md.j2"}"#).unwrap();
        assert!(Config::from_file(&path).is_err());
    }

    #[test]
    fn set_applies_known_keys_and_rejects_others() {
        let dir = tempfile::tempdir().unwrap();
        let template = dir.path().join("t.md.j2");
        fs::write(&template, "ok").unwrap();
        let mut config = Config::default();
        config
            .set(&format!("template={}", template.display()).parse().unwrap())
            .unwrap();
        config.set(&"comment-key=prod".parse().unwrap()).unwrap();
        assert_eq!(config.template, Some(fs::canonicalize(&template).unwrap()));
        assert_eq!(config.comment_key.as_deref(), Some("prod"));

        assert!(config.set(&"colour=blue".parse().unwrap()).is_err());
        assert!(config.set(&"comment-key=".parse().unwrap()).is_err());
        assert!("no-equals".parse::<Setting>().is_err());
    }

    #[test]
    fn invalid_json_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, "not json").unwrap();
        assert!(Config::load(&path).is_err());
    }
}

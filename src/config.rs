//! Per-job settings that later calls read back: the config file.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::str::FromStr;

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

/// How each Built-in Status is shown until `config set status.<name>=…` changes it.
const BUILT_IN_STATUS_LABELS: [(&str, &str); 6] = [
    ("pending", "⬜ Pending"),
    ("running", "⏳ Running…"),
    ("success", "✅ Success"),
    ("failure", "❌ Failed"),
    ("skipped", "⏭️ Skipped"),
    ("cancelled", "🚫 Cancelled"),
];

/// `rules-check` → `Rules Check`.
fn display_name_for(name: &str) -> String {
    name.split(['-', '_', ' '])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect(),
                None => String::new(),
            }
        })
        .collect::<Vec<String>>()
        .join(" ")
}

/// The `template` value meaning the built-in Template.
const DEFAULT_TEMPLATE: &str = "default";

/// An empty value removes the key, so the fallback applies again.
fn set_or_remove(map: &mut BTreeMap<String, String>, key: &str, value: &str) {
    if value.is_empty() {
        map.remove(key);
    } else {
        map.insert(key.to_string(), value.to_string());
    }
}

/// Keys are spelled as `config set` takes them. Every command fills in the
/// defaults and writes them back, so the file shows what's in effect.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Config {
    /// `default` for the built-in Template, or a Template file's absolute path.
    #[serde(default)]
    pub template: String,
    /// Empty only when there's no default to give it.
    #[serde(
        default,
        rename = "comment-key",
        skip_serializing_if = "String::is_empty"
    )]
    pub comment_key: String,
    /// How each Status is shown, by Status: `config set status.<status>=<label>`.
    #[serde(default, rename = "status", skip_serializing_if = "BTreeMap::is_empty")]
    pub status_labels: BTreeMap<String, String>,
    /// Display names, by Step name: `config set step.<name>=<display name>`.
    #[serde(default, rename = "step", skip_serializing_if = "BTreeMap::is_empty")]
    pub step_names: BTreeMap<String, String>,
    /// GitHub token; beats `$GITHUB_TOKEN` but not `--token`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub token: String,
}

/// A `config set` argument: `KEY=VALUE`.
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
    /// The saved config, or an empty one when there's none yet.
    pub fn read(path: &Path) -> Result<Config> {
        match fs::read_to_string(path) {
            Ok(json) => parse(&json, path),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(e).with_context(|| format!("reading config {}", path.display())),
        }
    }

    /// Reads a config supplied by `config load`. A relative Template path is
    /// resolved against the file's directory.
    pub fn load(path: &Path) -> Result<Config> {
        let json = fs::read_to_string(path)
            .with_context(|| format!("reading config {}", path.display()))?;
        let mut config = parse(&json, path)?;
        if !config.template.is_empty() && config.template != DEFAULT_TEMPLATE {
            let base = path.parent().unwrap_or(Path::new("."));
            config.template = template_path(&base.join(&config.template))?;
        }
        Ok(config)
    }

    /// Fills unset keys with their defaults. The Comment Key stays empty when
    /// there's no default to give it.
    pub fn fill_defaults(&mut self, comment_key: Option<String>) {
        if self.template.is_empty() {
            self.template = DEFAULT_TEMPLATE.into();
        }
        if self.comment_key.is_empty() {
            self.comment_key = comment_key.unwrap_or_default();
        }
        for (status, label) in BUILT_IN_STATUS_LABELS {
            self.status_labels
                .entry(status.into())
                .or_insert_with(|| label.into());
        }
    }

    /// The Template file's path, or `None` for the built-in Template.
    pub fn template_file(&self) -> Option<&Path> {
        (self.template != DEFAULT_TEMPLATE).then(|| Path::new(&self.template))
    }

    /// How a Status is shown: its configured label, or the Status itself.
    pub fn status_label(&self, status: &str) -> String {
        self.status_labels
            .get(status)
            .cloned()
            .unwrap_or_else(|| status.to_string())
    }

    /// A Step's display name: the configured one, or the title-cased name.
    pub fn step_name(&self, name: &str) -> String {
        self.step_names
            .get(name)
            .cloned()
            .unwrap_or_else(|| display_name_for(name))
    }

    /// Applies one `config set` value; an empty one resets the key to its default.
    /// A relative Template path resolves against the current directory.
    pub fn set(&mut self, setting: &Setting) -> Result<()> {
        let value = setting.value.as_str();
        match setting.key.as_str() {
            "template" if value.is_empty() || value == DEFAULT_TEMPLATE => {
                self.template = value.into()
            }
            "template" => self.template = template_path(Path::new(value))?,
            "comment-key" => self.comment_key = value.into(),
            "token" => self.token = value.into(),
            key => match key.split_once('.') {
                Some(("status", status)) => set_or_remove(&mut self.status_labels, status, value),
                Some(("step", step)) => set_or_remove(&mut self.step_names, step, value),
                _ => bail!(
                    "unknown config key {key:?}; known keys: template, comment-key, token, status.<status>, step.<step>"
                ),
            },
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

fn parse(json: &str, path: &Path) -> Result<Config> {
    serde_json::from_str(json)
        .with_context(|| format!("config {} is not valid JSON", path.display()))
}

/// The Template's absolute path, once it's known to exist.
fn template_path(path: &Path) -> Result<String> {
    let path =
        fs::canonicalize(path).with_context(|| format!("reading Template {}", path.display()))?;
    Ok(path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_names_are_title_cased() {
        assert_eq!(display_name_for("rules-check"), "Rules Check");
        assert_eq!(display_name_for("plan"), "Plan");
        assert_eq!(display_name_for("bar step"), "Bar Step");
        assert_eq!(display_name_for("tf_apply"), "Tf Apply");
    }
}

//! Per-job settings that later calls read back: the config file.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
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

/// An empty value removes the key, so the fallback applies again.
fn set_or_remove(map: &mut BTreeMap<String, String>, key: &str, value: Option<String>) {
    match value {
        Some(value) => map.insert(key.to_string(), value),
        None => map.remove(key),
    };
}

/// Which Template renders the Status Comment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum TemplateSetting {
    /// The built-in Template, spelled `default`.
    Default,
    /// A Template file, by absolute path.
    File(PathBuf),
}

impl From<String> for TemplateSetting {
    fn from(s: String) -> Self {
        if s == "default" {
            TemplateSetting::Default
        } else {
            TemplateSetting::File(s.into())
        }
    }
}

impl From<TemplateSetting> for String {
    fn from(t: TemplateSetting) -> String {
        match t {
            TemplateSetting::Default => "default".into(),
            TemplateSetting::File(path) => path.display().to_string(),
        }
    }
}

/// Keys are spelled as `config set` takes them. Every command fills in the
/// defaults and writes them back, so the file shows what's in effect.
#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<TemplateSetting>,
    #[serde(
        default,
        rename = "comment-key",
        skip_serializing_if = "Option::is_none"
    )]
    pub comment_key: Option<String>,
    /// How each Status is shown, by Status: `config set status.<status>=<label>`.
    #[serde(default, rename = "status", skip_serializing_if = "BTreeMap::is_empty")]
    pub status_labels: BTreeMap<String, String>,
    /// Display names, by Step name: `config set step.<name>=<display name>`.
    #[serde(default, rename = "step", skip_serializing_if = "BTreeMap::is_empty")]
    pub step_names: BTreeMap<String, String>,
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
    /// `None` when the file doesn't exist yet.
    pub fn read(path: &Path) -> Result<Option<Config>> {
        match fs::read_to_string(path) {
            Ok(json) => serde_json::from_str(&json)
                .map(Some)
                .with_context(|| format!("config {} is not valid JSON", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("reading config {}", path.display())),
        }
    }

    /// Reads a config supplied by `config load`. Relative Template paths are
    /// resolved against the file's directory, and the Template is checked.
    pub fn from_file(path: &Path) -> Result<Config> {
        let mut config =
            Config::read(path)?.ok_or_else(|| anyhow!("config {} not found", path.display()))?;
        if let Some(TemplateSetting::File(template)) = &config.template {
            let base = path.parent().unwrap_or(Path::new("."));
            config.template = Some(TemplateSetting::File(check_template(&base.join(template))?));
        }
        Ok(config)
    }

    /// Fills unset keys with their defaults. Returns whether anything changed.
    /// The Comment Key stays unset when there's no default to give it.
    pub fn fill_defaults(&mut self, comment_key: Option<String>) -> bool {
        let mut changed = false;
        if self.template.is_none() {
            self.template = Some(TemplateSetting::Default);
            changed = true;
        }
        if self.comment_key.is_none() && comment_key.is_some() {
            self.comment_key = comment_key;
            changed = true;
        }
        for (status, label) in BUILT_IN_STATUS_LABELS {
            if !self.status_labels.contains_key(status) {
                self.status_labels.insert(status.into(), label.into());
                changed = true;
            }
        }
        changed
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

    /// Applies one `config set` value. A relative Template path resolves against
    /// the current directory.
    pub fn set(&mut self, setting: &Setting) -> Result<()> {
        let value = (!setting.value.is_empty()).then(|| setting.value.clone());
        match setting.key.as_str() {
            "template" | "comment-key" if value.is_none() => {
                bail!("`{}` needs a value", setting.key)
            }
            "template" if setting.value == "default" => {
                self.template = Some(TemplateSetting::Default)
            }
            "template" => {
                self.template = Some(TemplateSetting::File(check_template(Path::new(
                    &setting.value,
                ))?))
            }
            "comment-key" => self.comment_key = Some(setting.value.clone()),
            key => match key.split_once('.') {
                Some(("status", status)) if !status.is_empty() => {
                    set_or_remove(&mut self.status_labels, status, value)
                }
                Some(("step", step)) if !step.is_empty() => {
                    set_or_remove(&mut self.step_names, step, value)
                }
                _ => bail!(
                    "unknown config key {key:?}; known keys: template, comment-key, status.<status>, step.<step>"
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
    fn missing_file_reads_as_none_and_saved_config_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/config.json");
        assert_eq!(Config::read(&path).unwrap(), None);

        let config = Config {
            template: Some(TemplateSetting::File("/repo/t.md.j2".into())),
            comment_key: Some("prod".into()),
            ..Config::default()
        };
        config.save(&path).unwrap();
        assert_eq!(Config::read(&path).unwrap(), Some(config));
    }

    #[test]
    fn fill_defaults_sets_only_missing_keys() {
        let mut config = Config::default();
        assert!(config.fill_defaults(Some("wf/job".into())));
        assert_eq!(config.template, Some(TemplateSetting::Default));
        assert_eq!(config.comment_key.as_deref(), Some("wf/job"));
        assert!(!config.fill_defaults(Some("other".into())));
        assert_eq!(config.comment_key.as_deref(), Some("wf/job"));
    }

    #[test]
    fn template_default_round_trips_as_a_string() {
        let config = Config {
            template: Some(TemplateSetting::Default),
            ..Config::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        assert_eq!(json, r#"{"template":"default"}"#);
        assert_eq!(serde_json::from_str::<Config>(&json).unwrap(), config);
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
            Some(TemplateSetting::File(
                fs::canonicalize(dir.path().join("ci/t.md.j2")).unwrap()
            ))
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
        assert_eq!(
            config.template,
            Some(TemplateSetting::File(fs::canonicalize(&template).unwrap()))
        );
        assert_eq!(config.comment_key.as_deref(), Some("prod"));

        config.set(&"template=default".parse().unwrap()).unwrap();
        assert_eq!(config.template, Some(TemplateSetting::Default));

        assert!(config.set(&"colour=blue".parse().unwrap()).is_err());
        assert!(config.set(&"comment-key=".parse().unwrap()).is_err());
        assert!("no-equals".parse::<Setting>().is_err());
    }

    #[test]
    fn built_in_status_labels_are_defaults_and_can_change() {
        let mut config = Config::default();
        config.fill_defaults(None);
        assert_eq!(config.status_label("failure"), "❌ Failed");
        assert_eq!(config.status_label("changes"), "changes");

        config
            .set(&"status.changes=🔍 Changes detected".parse().unwrap())
            .unwrap();
        config
            .set(&"status.failure=💥 Broken".parse().unwrap())
            .unwrap();
        assert_eq!(config.status_label("changes"), "🔍 Changes detected");
        assert!(
            !config.fill_defaults(None),
            "a changed default stays changed"
        );
        assert_eq!(config.status_label("failure"), "💥 Broken");
    }

    #[test]
    fn display_names_are_title_cased() {
        assert_eq!(display_name_for("rules-check"), "Rules Check");
        assert_eq!(display_name_for("plan"), "Plan");
        assert_eq!(display_name_for("bar step"), "Bar Step");
        assert_eq!(display_name_for("tf_apply"), "Tf Apply");
    }

    #[test]
    fn step_display_names_default_to_title_case() {
        let mut config = Config::default();
        assert_eq!(config.step_name("rules-check"), "Rules Check");
        config
            .set(&"step.rules-check=OPA Rules".parse().unwrap())
            .unwrap();
        assert_eq!(config.step_name("rules-check"), "OPA Rules");
        assert!(config.set(&"step.=x".parse().unwrap()).is_err());
        assert!(config.set(&"steps.x=y".parse().unwrap()).is_err());
    }

    #[test]
    fn invalid_json_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, "not json").unwrap();
        assert!(Config::read(&path).is_err());
    }
}

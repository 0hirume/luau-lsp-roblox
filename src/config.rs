use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::{Result, error};

const COMPATIBILITY_JSON: &str = include_str!("../upstream/compatibility.json");
#[cfg(test)]
const SCHEMA_JSON: &str = include_str!("../upstream/schema.json");

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Class {
    Forwarded,
    Adapted,
    Partial,
    Unsupported,
}

#[derive(Clone, Debug, Deserialize)]
struct Entry {
    class: Class,
    reason: String,
}

#[derive(Debug, Deserialize)]
struct Compatibility {
    upstream_version: String,
    upstream_commit: String,
    settings: BTreeMap<String, Entry>,
}

#[derive(Clone, Debug, Default)]
pub struct Settings {
    flat: BTreeMap<String, Value>,
}

impl Settings {
    pub(crate) fn defaults() -> Self {
        let flat = BTreeMap::from([
            (
                "luau-lsp.platform.type".to_owned(),
                Value::String("roblox".to_owned()),
            ),
            ("luau-lsp.sourcemap.enabled".to_owned(), Value::Bool(true)),
            (
                "luau-lsp.sourcemap.autogenerate".to_owned(),
                Value::Bool(true),
            ),
            (
                "luau-lsp.sourcemap.rojoProjectFile".to_owned(),
                Value::String("default.project.json".to_owned()),
            ),
            (
                "luau-lsp.sourcemap.includeNonScripts".to_owned(),
                Value::Bool(true),
            ),
            (
                "luau-lsp.sourcemap.sourcemapFile".to_owned(),
                Value::String("sourcemap.json".to_owned()),
            ),
            (
                "luau-lsp.sourcemap.useVSCodeWatcher".to_owned(),
                Value::Bool(false),
            ),
            (
                "luau-lsp.fflags.enableByDefault".to_owned(),
                Value::Bool(false),
            ),
            (
                "luau-lsp.fflags.enableNewSolver".to_owned(),
                Value::Bool(false),
            ),
            ("luau-lsp.fflags.sync".to_owned(), Value::Bool(true)),
            ("luau-lsp.types.roblox".to_owned(), Value::Bool(true)),
            (
                "luau-lsp.types.robloxSecurityLevel".to_owned(),
                Value::String("PluginSecurity".to_owned()),
            ),
            (
                "luau-lsp.studioPlugin.enabled".to_owned(),
                Value::Bool(false),
            ),
        ]);
        Self { flat }
    }

    pub(crate) fn from_path(path: &Path) -> Result<Self> {
        let contents = fs::read_to_string(path).map_err(|source| {
            error(format!(
                "failed to read settings {}: {source}",
                path.display()
            ))
        })?;
        let value: Value = serde_json::from_str(&contents).map_err(|source| {
            error(format!(
                "failed to parse settings {}: {source}",
                path.display()
            ))
        })?;
        Self::from_value(&value)
    }

    pub(crate) fn from_value(value: &Value) -> Result<Self> {
        let compatibility = compatibility()?;
        let mut settings = Self::default();
        let root = value.get("settings").unwrap_or(value);
        let object = root
            .as_object()
            .ok_or_else(|| error("settings must be a JSON object"))?;

        for (key, value) in object {
            let key = if key == "luau-lsp"
                || key == "luau"
                || key.starts_with("luau-lsp.")
                || key.starts_with("luau.")
            {
                key.clone()
            } else {
                format!("luau-lsp.{key}")
            };
            collect(&mut settings.flat, &compatibility, key, value.clone());
        }
        Ok(settings)
    }

    pub(crate) fn merge(&mut self, other: Self) {
        self.flat.extend(other.flat);
    }

    pub(crate) fn set(&mut self, key: impl Into<String>, value: Value) {
        self.flat.insert(key.into(), value);
    }

    pub(crate) fn get(&self, key: &str) -> Option<&Value> {
        self.flat.get(key)
    }

    pub(crate) fn boolean(&self, key: &str, default: bool) -> bool {
        self.get(key).and_then(Value::as_bool).unwrap_or(default)
    }

    pub(crate) fn string(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Value::as_str)
    }

    pub(crate) fn integer(&self, key: &str) -> Option<u64> {
        self.get(key).and_then(Value::as_u64)
    }

    pub(crate) fn object(&self, key: &str) -> Option<&Map<String, Value>> {
        self.get(key).and_then(Value::as_object)
    }

    pub(crate) fn dotted(&self) -> Value {
        Value::Object(self.flat.clone().into_iter().collect())
    }

    pub(crate) fn server_object(&self) -> Value {
        let mut output = Map::new();
        for (key, value) in &self.flat {
            if let Some(path) = key.strip_prefix("luau-lsp.") {
                insert_nested(&mut output, path, value.clone());
            }
        }
        Value::Object(output)
    }

    pub(crate) fn notices(&self) -> Result<Vec<String>> {
        let compatibility = compatibility()?;
        let mut notices = Vec::new();
        for key in self.flat.keys() {
            match compatibility.settings.get(key) {
                Some(entry) if entry.class == Class::Partial => notices.push(format!(
                    "{key} is partially portable with upstream {}: {}",
                    compatibility.upstream_version, entry.reason
                )),
                Some(entry) if entry.class == Class::Unsupported => notices.push(format!(
                    "{key} is unsupported by the editor-neutral wrapper for upstream {}: {}",
                    compatibility.upstream_version, entry.reason
                )),
                None if key.starts_with("luau-lsp.") || key.starts_with("luau.") => {
                    notices.push(format!(
                        "{key} is not classified for upstream {} ({}) and will only be forwarded",
                        compatibility.upstream_version, compatibility.upstream_commit
                    ));
                }
                _ => {}
            }
        }
        Ok(notices)
    }
}

pub fn normalize_section(value: &Value, baseline: &Settings) -> Result<Value> {
    if value.is_null() {
        return Ok(baseline.server_object());
    }
    let mut merged = baseline.clone();
    merged.merge(Settings::from_value(value)?);
    if let Some(platform) = baseline.get("luau-lsp.platform.type").cloned() {
        merged.set("luau-lsp.platform.type", platform);
    }
    Ok(merged.server_object())
}

pub fn normalize_change(message: &mut Value, baseline: &Settings) -> Result<Vec<String>> {
    let Some(settings) = message.pointer("/params/settings").cloned() else {
        return Ok(Vec::new());
    };
    let changed = Settings::from_value(&settings)?;
    let notices = changed.notices()?;
    let normalized = normalize_section(&settings, baseline)?;
    if let Some(slot) = message.pointer_mut("/params/settings") {
        *slot = normalized;
    }
    Ok(notices)
}

pub fn normalize_response(message: &mut Value, baseline: &Settings) -> Result<()> {
    let Some(result) = message.get_mut("result").and_then(Value::as_array_mut) else {
        return Ok(());
    };
    for value in result {
        *value = normalize_section(value, baseline)?;
    }
    Ok(())
}

fn collect(
    output: &mut BTreeMap<String, Value>,
    compatibility: &Compatibility,
    key: String,
    value: Value,
) {
    if compatibility.settings.contains_key(&key) || !value.is_object() {
        output.insert(key, value);
        return;
    }

    let Value::Object(object) = value else {
        return;
    };
    for (child, value) in object {
        collect(output, compatibility, format!("{key}.{child}"), value);
    }
}

fn insert_nested(output: &mut Map<String, Value>, path: &str, value: Value) {
    let mut parts = path.split('.').peekable();
    let mut current = output;
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            current.insert(part.to_owned(), value);
            return;
        }
        let entry = current
            .entry(part.to_owned())
            .or_insert_with(|| Value::Object(Map::new()));
        if !entry.is_object() {
            *entry = Value::Object(Map::new());
        }
        let Some(object) = entry.as_object_mut() else {
            return;
        };
        current = object;
    }
}

fn compatibility() -> Result<Compatibility> {
    serde_json::from_str(COMPATIBILITY_JSON)
        .map_err(|source| error(format!("invalid embedded compatibility registry: {source}")))
}

#[cfg(test)]
mod tests {
    use super::{COMPATIBILITY_JSON, SCHEMA_JSON, Settings, normalize_response};
    use crate::Result;
    use serde::Deserialize;
    use serde_json::{Value, json};
    use std::collections::{BTreeMap, BTreeSet};

    #[derive(Deserialize)]
    struct Schema {
        upstream_version: String,
        upstream_commit: String,
        settings: BTreeMap<String, Value>,
    }

    #[derive(Deserialize)]
    struct Registry {
        upstream_version: String,
        upstream_commit: String,
        settings: BTreeMap<String, Value>,
    }

    #[test]
    fn every_upstream_setting_is_explicitly_classified() -> Result<()> {
        let manifest = crate::assets::upstream()?;
        let schema: Schema = serde_json::from_str(SCHEMA_JSON)?;
        let registry: Registry = serde_json::from_str(COMPATIBILITY_JSON)?;
        let schema_keys: BTreeSet<_> = schema.settings.into_keys().collect();
        let registry_keys: BTreeSet<_> = registry.settings.into_keys().collect();

        assert_eq!(schema.upstream_version, manifest.version);
        assert_eq!(schema.upstream_commit, manifest.commit);
        assert_eq!(schema.upstream_version, registry.upstream_version);
        assert_eq!(schema.upstream_commit, registry.upstream_commit);
        assert!(!schema_keys.is_empty());
        assert_eq!(schema_keys, registry_keys);
        Ok(())
    }

    #[test]
    fn nested_and_dotted_shapes_normalize_identically() -> Result<()> {
        let nested = Settings::from_value(&json!({
            "luau-lsp": { "hover": { "enabled": false } },
            "luau-lsp.fflags.override": { "LuauSolverV2": "true" }
        }))?;

        assert_eq!(
            nested.get("luau-lsp.hover.enabled"),
            Some(&Value::Bool(false))
        );
        assert!(nested.object("luau-lsp.fflags.override").is_some());
        Ok(())
    }

    #[test]
    fn configuration_responses_include_wrapper_baseline() -> Result<()> {
        let mut message = json!({ "jsonrpc": "2.0", "id": 7, "result": [
            { "hover": { "enabled": false }, "platform": { "type": "standard" } }
        ] });
        normalize_response(&mut message, &Settings::defaults())?;

        assert_eq!(
            message.pointer("/result/0/platform/type"),
            Some(&json!("roblox"))
        );
        assert_eq!(
            message.pointer("/result/0/hover/enabled"),
            Some(&json!(false))
        );
        Ok(())
    }

    #[test]
    fn managed_roblox_defaults_need_no_options() {
        let settings = Settings::defaults();

        assert_eq!(settings.string("luau-lsp.platform.type"), Some("roblox"));
        assert_eq!(
            settings.string("luau-lsp.types.robloxSecurityLevel"),
            Some("PluginSecurity")
        );
        assert!(settings.boolean("luau-lsp.types.roblox", false));
        assert!(settings.boolean("luau-lsp.fflags.sync", false));
        assert!(settings.boolean("luau-lsp.sourcemap.enabled", false));
        assert!(settings.boolean("luau-lsp.sourcemap.autogenerate", false));
    }
}

use std::collections::BTreeSet;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::{Result, error};

const MAX_MESSAGE_SIZE: usize = 64 * 1024 * 1024;
const MAX_HEADER_SIZE: usize = 16 * 1024;

#[derive(Debug)]
pub struct Initialize {
    pub message: Value,
    pub roots: Vec<PathBuf>,
    pub settings: Option<Value>,
    pub flags: Option<Map<String, Value>>,
}

impl Initialize {
    pub(crate) fn parse(message: Value) -> Result<Self> {
        if message.get("method").and_then(Value::as_str) != Some("initialize") {
            return Err(error(
                "managed Roblox mode expected initialize as the first LSP message",
            ));
        }

        let roots = workspace_roots(&message);
        let options = message.pointer("/params/initializationOptions");
        let settings = options
            .and_then(|value| value.get("settings"))
            .cloned()
            .or_else(|| options.and_then(extract_embedded_settings));
        let flags = options
            .and_then(|value| value.get("fflags"))
            .and_then(Value::as_object)
            .cloned();

        Ok(Self {
            message,
            roots,
            settings,
            flags,
        })
    }

    pub(crate) fn set_flags(&mut self, flags: &std::collections::BTreeMap<String, String>) {
        let params = object_at(&mut self.message, "params");
        let options = object_at_value(params, "initializationOptions");
        options.insert(
            "fflags".to_owned(),
            Value::Object(
                flags
                    .iter()
                    .map(|(name, value)| (name.clone(), Value::String(value.clone())))
                    .collect(),
            ),
        );
    }
}

pub fn read<R: BufRead>(input: &mut R) -> Result<Option<Value>> {
    let mut content_length = None;
    let mut consumed = 0_usize;
    loop {
        let mut line = String::new();
        let bytes = input.read_line(&mut line)?;
        if bytes == 0 {
            if consumed == 0 {
                return Ok(None);
            }
            return Err(error("unexpected EOF in LSP headers"));
        }
        consumed = consumed.saturating_add(bytes);
        if consumed > MAX_HEADER_SIZE {
            return Err(error("LSP headers exceed the size limit"));
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("Content-Length") {
                content_length = Some(value.trim().parse::<usize>()?);
            }
        }
    }

    let length = content_length.ok_or_else(|| error("LSP message has no Content-Length header"))?;
    if length > MAX_MESSAGE_SIZE {
        return Err(error(format!(
            "LSP message exceeds {MAX_MESSAGE_SIZE} bytes"
        )));
    }
    let mut body = vec![0_u8; length];
    input.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|source| error(format!("invalid LSP JSON: {source}")))
}

pub fn write<W: Write>(output: &mut W, message: &Value) -> Result<()> {
    let body = serde_json::to_vec(message)?;
    write!(output, "Content-Length: {}\r\n\r\n", body.len())?;
    output.write_all(&body)?;
    output.flush()?;
    Ok(())
}

pub fn log(level: u8, message: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "window/logMessage",
        "params": { "type": level, "message": message.into() }
    })
}

pub fn sourcemap_changed(path: &Path) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "workspace/didChangeWatchedFiles",
        "params": { "changes": [{ "uri": file_uri(path), "type": 2 }] }
    })
}

pub fn plugin_full(tree: &Value) -> Value {
    json!({ "jsonrpc": "2.0", "method": "$/plugin/full", "params": tree })
}

pub fn plugin_clear() -> Value {
    json!({ "jsonrpc": "2.0", "method": "$/plugin/clear" })
}

pub fn id_key(message: &Value) -> Option<String> {
    message
        .get("id")
        .and_then(|id| serde_json::to_string(id).ok())
}

fn workspace_roots(message: &Value) -> Vec<PathBuf> {
    let mut roots = BTreeSet::new();
    if let Some(folders) = message
        .pointer("/params/workspaceFolders")
        .and_then(Value::as_array)
    {
        for folder in folders {
            if let Some(uri) = folder
                .get("uri")
                .and_then(Value::as_str)
                .and_then(file_path)
            {
                roots.insert(uri);
            }
        }
    }
    if roots.is_empty() {
        if let Some(path) = message
            .pointer("/params/rootUri")
            .and_then(Value::as_str)
            .and_then(file_path)
        {
            roots.insert(path);
        } else if let Some(path) = message.pointer("/params/rootPath").and_then(Value::as_str) {
            roots.insert(PathBuf::from(path));
        }
    }
    roots.into_iter().collect()
}

fn extract_embedded_settings(options: &Value) -> Option<Value> {
    let object = options.as_object()?;
    let filtered: Map<String, Value> = object
        .iter()
        .filter(|(key, _value)| {
            *key == "luau-lsp"
                || *key == "luau"
                || key.starts_with("luau-lsp.")
                || key.starts_with("luau.")
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    (!filtered.is_empty()).then_some(Value::Object(filtered))
}

fn object_at<'a>(value: &'a mut Value, key: &str) -> &'a mut Map<String, Value> {
    if !value.is_object() {
        *value = Value::Object(Map::new());
    }
    let Some(object) = value.as_object_mut() else {
        unreachable_non_object()
    };
    object_at_value(object, key)
}

fn object_at_value<'a>(
    object: &'a mut Map<String, Value>,
    key: &str,
) -> &'a mut Map<String, Value> {
    let value = object
        .entry(key.to_owned())
        .or_insert_with(|| Value::Object(Map::new()));
    if !value.is_object() {
        *value = Value::Object(Map::new());
    }
    let Some(result) = value.as_object_mut() else {
        unreachable_non_object()
    };
    result
}

fn unreachable_non_object() -> ! {
    std::process::abort()
}

fn file_path(uri: &str) -> Option<PathBuf> {
    let encoded = uri.strip_prefix("file://")?;
    let mut path = percent_decode(encoded)?;
    if cfg!(windows) && path.starts_with('/') && path.as_bytes().get(2) == Some(&b':') {
        path.remove(0);
    }
    Some(PathBuf::from(path))
}

fn file_uri(path: &Path) -> String {
    let path = path.to_string_lossy().replace('\\', "/");
    let prefix = if path.starts_with('/') {
        "file://"
    } else {
        "file:///"
    };
    format!("{prefix}{}", percent_encode(&path))
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while let Some(byte) = bytes.get(index).copied() {
        if byte == b'%' {
            let high = *bytes.get(index + 1)?;
            let low = *bytes.get(index + 2)?;
            output.push(hex(high)? * 16 + hex(low)?);
            index += 3;
        } else {
            output.push(byte);
            index += 1;
        }
    }
    String::from_utf8(output).ok()
}

const fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn percent_encode(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b':' | b'-' | b'_' | b'.' | b'~') {
            output.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            let _result = write!(output, "%{byte:02X}");
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::{Initialize, file_path, file_uri, read, write};
    use crate::Result;
    use serde_json::json;
    use std::io::{BufReader, Cursor};
    use std::path::Path;

    #[test]
    fn message_framing_round_trips() -> Result<()> {
        let message = json!({"jsonrpc": "2.0", "method": "initialized", "params": {}});
        let mut bytes = Vec::new();
        write(&mut bytes, &message)?;
        let mut reader = BufReader::new(Cursor::new(bytes));
        assert_eq!(read(&mut reader)?, Some(message));
        Ok(())
    }

    #[test]
    fn initialize_extracts_roots_and_settings() -> Result<()> {
        let initialize = Initialize::parse(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "rootUri": "file:///work/game",
                "initializationOptions": {
                    "settings": { "luau-lsp.fflags.sync": false },
                    "fflags": { "LuauSolverV2": "true" }
                }
            }
        }))?;
        assert_eq!(initialize.roots, [Path::new("/work/game")]);
        assert!(initialize.settings.is_some());
        assert!(initialize.flags.is_some());
        Ok(())
    }

    #[test]
    fn file_uris_encode_spaces() {
        let path = Path::new("/work/my game/sourcemap.json");
        let uri = file_uri(path);
        assert_eq!(uri, "file:///work/my%20game/sourcemap.json");
        assert_eq!(file_path(&uri).as_deref(), Some(path));
    }
}

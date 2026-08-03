use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{BufReader, BufWriter, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use serde_json::Value;

use crate::args::Args;
use crate::assets::{self, Security};
use crate::config::{Settings, normalize_change, normalize_response};
use crate::lsp::{self, Initialize};
use crate::services;
use crate::{Result, error};

/// Runs the wrapper and returns its process exit code.
///
/// # Errors
///
/// Returns an error when arguments, configuration, resources, or a child process
/// cannot be prepared safely.
pub fn run() -> Result<u8> {
    let arguments = Args::parse()?;
    if arguments.help {
        std::io::stdout()
            .lock()
            .write_all(arguments.help_text().as_bytes())?;
        return Ok(0);
    }
    if arguments.version {
        let upstream = assets::upstream()?;
        writeln!(
            std::io::stdout().lock(),
            "luau-lsp wrapper {} (upstream {} {})",
            env!("CARGO_PKG_VERSION"),
            upstream.version,
            upstream.commit
        )?;
        return Ok(0);
    }

    let upstream = arguments.upstream_path()?;
    if arguments.managed() {
        managed(arguments, &upstream)
    } else {
        transparent(&arguments.forwarded, &upstream)
    }
}

fn transparent(arguments: &[OsString], upstream: &Path) -> Result<u8> {
    let status = Command::new(upstream)
        .args(arguments)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()?;
    Ok(exit_code(status.code()))
}

fn managed(mut arguments: Args, upstream_path: &Path) -> Result<u8> {
    reject_pipe(&arguments.forwarded)?;
    let mut input = BufReader::new(std::io::stdin());
    let first =
        lsp::read(&mut input)?.ok_or_else(|| error("LSP stdin closed before initialize"))?;
    let mut initialize = Initialize::parse(first)?;

    let prepared = prepare_session(&mut arguments, &mut initialize, upstream_path)?;

    let mut process = start_process(upstream_path, &arguments.forwarded)?;
    let client_done = Arc::new(AtomicBool::new(false));
    if let Err(source) = process.upstream.send(initialize.message) {
        terminate_process(&mut process);
        return Err(source.into());
    }
    for warning in prepared.warnings {
        let _result = process.client.send(lsp::log(2, warning));
    }

    let service_handles = match services::start(
        &prepared.settings,
        &initialize.roots,
        &process.upstream,
        &process.client,
        &process.stop,
    ) {
        Ok(handles) => handles,
        Err(source) => {
            terminate_process(&mut process);
            return Err(source);
        }
    };

    let client_reader = start_client_reader(
        input,
        &process,
        prepared.settings.clone(),
        Arc::clone(&client_done),
    );

    let mut status = None;
    while status.is_none()
        && !client_done.load(Ordering::Acquire)
        && !process.stop.load(Ordering::Acquire)
    {
        status = process.child.try_wait()?;
        thread::sleep(Duration::from_millis(25));
    }

    process.stop.store(true, Ordering::Release);
    for handle in service_handles {
        let _result = handle.join();
    }
    drop(process.upstream);
    let _result = process.upstream_writer.join();

    if status.is_none() {
        let deadline = Instant::now() + Duration::from_secs(2);
        while status.is_none() && Instant::now() < deadline {
            status = process.child.try_wait()?;
            thread::sleep(Duration::from_millis(25));
        }
    }
    if status.is_none() {
        process.child.kill()?;
        status = Some(process.child.wait()?);
    }

    let normal_shutdown = client_done.load(Ordering::Acquire);
    if normal_shutdown {
        let _result = client_reader.join();
    }
    let _result = process.server_reader.join();
    drop(process.client);
    if normal_shutdown {
        let _result = process.client_writer.join();
    }
    drop(prepared.file);

    Ok(status.map_or(1, |status| exit_code(status.code())))
}

fn start_client_reader(
    mut input: BufReader<std::io::Stdin>,
    process: &ProcessIo,
    baseline: Settings,
    done: Arc<AtomicBool>,
) -> thread::JoinHandle<()> {
    let upstream = process.upstream.clone();
    let client = process.client.clone();
    let pending = Arc::clone(&process.pending);
    thread::spawn(move || {
        loop {
            let mut message = match lsp::read(&mut input) {
                Ok(Some(message)) => message,
                Ok(None) => break,
                Err(source) => {
                    write_stderr(&format!("invalid client LSP input: {source}"));
                    break;
                }
            };
            normalize_client_message(&mut message, &pending, &client, &baseline);
            let exit = message.get("method").and_then(Value::as_str) == Some("exit");
            if upstream.send(message).is_err() || exit {
                break;
            }
        }
        done.store(true, Ordering::Release);
    })
}

fn normalize_client_message(
    message: &mut Value,
    pending: &Mutex<BTreeSet<String>>,
    client: &mpsc::Sender<Value>,
    baseline: &Settings,
) {
    if let Some(id) = lsp::id_key(message) {
        let tracked = pending
            .lock()
            .is_ok_and(|mut requests| requests.remove(&id));
        if tracked {
            if let Err(source) = normalize_response(message, baseline) {
                let _result = client.send(lsp::log(
                    1,
                    format!("failed to normalize workspace configuration: {source}"),
                ));
            }
        }
    }
    if message.get("method").and_then(Value::as_str) != Some("workspace/didChangeConfiguration") {
        return;
    }
    match normalize_change(message, baseline) {
        Ok(notices) => {
            for notice in notices {
                let _result = client.send(lsp::log(
                    2,
                    format!("{notice}; restart the LSP session for startup-owned changes"),
                ));
            }
        }
        Err(source) => {
            let _result = client.send(lsp::log(
                1,
                format!("failed to normalize changed configuration: {source}"),
            ));
        }
    }
}

struct ProcessIo {
    child: Child,
    stop: Arc<AtomicBool>,
    upstream: mpsc::Sender<Value>,
    client: mpsc::Sender<Value>,
    pending: Arc<Mutex<BTreeSet<String>>>,
    upstream_writer: thread::JoinHandle<()>,
    client_writer: thread::JoinHandle<()>,
    server_reader: thread::JoinHandle<()>,
}

fn terminate_process(process: &mut ProcessIo) {
    process.stop.store(true, Ordering::Release);
    if process.child.try_wait().ok().flatten().is_none() {
        let _result = process.child.kill();
        let _result = process.child.wait();
    }
}

fn start_process(upstream_path: &Path, arguments: &[OsString]) -> Result<ProcessIo> {
    let mut child = Command::new(upstream_path)
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    let child_stdin = child
        .stdin
        .take()
        .ok_or_else(|| error("failed to open bundled server stdin"))?;
    let child_stdout = child
        .stdout
        .take()
        .ok_or_else(|| error("failed to open bundled server stdout"))?;
    let stop = Arc::new(AtomicBool::new(false));
    let pending = Arc::new(Mutex::new(BTreeSet::<String>::new()));
    let (upstream, upstream_rx) = mpsc::channel::<Value>();
    let (client, client_rx) = mpsc::channel::<Value>();
    let upstream_writer = start_upstream_writer(child_stdin, upstream_rx);
    let client_writer = start_client_writer(client_rx, Arc::clone(&stop));
    let server_reader = start_server_reader(child_stdout, Arc::clone(&pending), client.clone());
    Ok(ProcessIo {
        child,
        stop,
        upstream,
        client,
        pending,
        upstream_writer,
        client_writer,
        server_reader,
    })
}

fn start_upstream_writer(
    child_stdin: ChildStdin,
    messages: mpsc::Receiver<Value>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut output = BufWriter::new(child_stdin);
        for message in messages {
            if lsp::write(&mut output, &message).is_err() {
                return;
            }
        }
    })
}

fn start_client_writer(
    messages: mpsc::Receiver<Value>,
    stop: Arc<AtomicBool>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut output = BufWriter::new(std::io::stdout());
        for message in messages {
            if lsp::write(&mut output, &message).is_err() {
                stop.store(true, Ordering::Release);
                return;
            }
        }
    })
}

fn start_server_reader(
    child_stdout: ChildStdout,
    pending: Arc<Mutex<BTreeSet<String>>>,
    client: mpsc::Sender<Value>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut input = BufReader::new(child_stdout);
        loop {
            match lsp::read(&mut input) {
                Ok(Some(message)) => {
                    track_configuration_request(&message, &pending);
                    if client.send(message).is_err() {
                        return;
                    }
                }
                Ok(None) => return,
                Err(source) => {
                    let _result = client.send(lsp::log(
                        1,
                        format!("bundled server produced invalid LSP: {source}"),
                    ));
                    return;
                }
            }
        }
    })
}

fn track_configuration_request(message: &Value, pending: &Mutex<BTreeSet<String>>) {
    if message.get("method").and_then(Value::as_str) == Some("workspace/configuration") {
        if let Some(id) = lsp::id_key(message) {
            if let Ok(mut requests) = pending.lock() {
                requests.insert(id);
            }
        }
    }
}

struct PreparedSession {
    settings: Settings,
    warnings: Vec<String>,
    file: SessionFile,
}

fn prepare_session(
    arguments: &mut Args,
    initialize: &mut Initialize,
    upstream_path: &Path,
) -> Result<PreparedSession> {
    let mut settings = Settings::defaults();
    if let Some(path) = take_settings_argument(&mut arguments.forwarded)? {
        settings.merge(Settings::from_path(&path)?);
    }
    if let Some(path) = &arguments.settings {
        settings.merge(Settings::from_path(path)?);
    }
    if let Some(value) = &initialize.settings {
        settings.merge(Settings::from_value(value)?);
    }
    settings.set("luau-lsp.platform.type", Value::String("roblox".to_owned()));
    if let Some(security) = &arguments.security {
        settings.set(
            "luau-lsp.types.robloxSecurityLevel",
            Value::String(security.clone()),
        );
    }
    if let Some(sync) = arguments.sync {
        settings.set("luau-lsp.fflags.sync", Value::Bool(sync));
    }
    if let Some(studio) = arguments.studio {
        settings.set("luau-lsp.studioPlugin.enabled", Value::Bool(studio));
    }

    let cache = arguments.cache_path()?;
    fs::create_dir_all(&cache)?;
    let resources = if settings.boolean("luau-lsp.types.roblox", true) {
        let security = Security::parse(
            settings
                .string("luau-lsp.types.robloxSecurityLevel")
                .unwrap_or("PluginSecurity"),
        )?;
        Some(assets::prepare(upstream_path, &cache, security)?)
    } else {
        None
    };
    let type_warnings = adapt_type_files(
        &mut arguments.forwarded,
        &mut settings,
        &cache,
        &initialize.roots,
    );
    let cli_flags = assets::take_cli_flags(&mut arguments.forwarded)?;
    let flags = assets::resolve_flags(
        upstream_path,
        &settings,
        initialize.flags.as_ref(),
        cli_flags,
    )?;
    initialize.set_flags(&flags.values);
    adapt_startup_arguments(
        &mut arguments.forwarded,
        &settings,
        &cache,
        resources.as_ref(),
    );
    let file = SessionFile::new(&cache, &settings.dotted())?;
    arguments.forwarded.push(OsString::from(format!(
        "--settings={}",
        file.path.display()
    )));
    let warnings = type_warnings
        .into_iter()
        .chain(settings.notices()?)
        .chain(flags.warnings)
        .collect();
    Ok(PreparedSession {
        settings,
        warnings,
        file,
    })
}

fn adapt_type_files(
    arguments: &mut Vec<OsString>,
    settings: &mut Settings,
    cache: &Path,
    roots: &[PathBuf],
) -> Vec<String> {
    let root = roots.first().map(PathBuf::as_path);
    let mut warnings = Vec::new();
    if let Some(value) = settings.get("luau-lsp.types.definitionFiles").cloned() {
        let definitions = configured_definitions(&value, cache, root, &mut warnings);
        for (package, path) in &definitions {
            arguments.push(OsString::from(format!("--definitions:{package}={path}")));
        }
        settings.set(
            "luau-lsp.types.definitionFiles",
            Value::Object(
                definitions
                    .into_iter()
                    .map(|(package, path)| (package, Value::String(path)))
                    .collect(),
            ),
        );
    }
    if let Some(value) = settings.get("luau-lsp.types.documentationFiles").cloned() {
        let documentation = configured_documentation(&value, cache, root, &mut warnings);
        for path in &documentation {
            arguments.push(OsString::from(format!("--docs={path}")));
        }
        settings.set(
            "luau-lsp.types.documentationFiles",
            Value::Array(documentation.into_iter().map(Value::String).collect()),
        );
    }
    warnings
}

fn configured_definitions(
    value: &Value,
    cache: &Path,
    root: Option<&Path>,
    warnings: &mut Vec<String>,
) -> BTreeMap<String, String> {
    let entries: Vec<(String, &Value)> = match value {
        Value::Object(values) => values
            .iter()
            .map(|(package, path)| (package.clone(), path))
            .collect(),
        Value::Array(values) => values
            .iter()
            .enumerate()
            .map(|(index, path)| (format!("roblox{index}"), path))
            .collect(),
        _ => {
            warnings.push(
                "types.definitionFiles must be a package-to-path object or path array".to_owned(),
            );
            return BTreeMap::new();
        }
    };
    entries
        .into_iter()
        .filter_map(|(package, value)| {
            if package.is_empty() || package.contains('=') {
                warnings.push(format!("ignored invalid definition package {package:?}"));
                return None;
            }
            let Some(path) = value.as_str() else {
                warnings.push(format!(
                    "ignored non-string definition path for package {package:?}"
                ));
                return None;
            };
            match resolve_type_file(path, cache, root, "definition", "d.luau") {
                Ok(path) => Some((package, path.to_string_lossy().into_owned())),
                Err(source) => {
                    warnings.push(format!(
                        "could not load definition package {package:?} from {path:?}: {source}"
                    ));
                    None
                }
            }
        })
        .collect()
}

fn configured_documentation(
    value: &Value,
    cache: &Path,
    root: Option<&Path>,
    warnings: &mut Vec<String>,
) -> Vec<String> {
    let Some(values) = value.as_array() else {
        warnings.push("types.documentationFiles must be a path array".to_owned());
        return Vec::new();
    };
    values
        .iter()
        .filter_map(|value| {
            let Some(path) = value.as_str() else {
                warnings.push("ignored non-string documentation path".to_owned());
                return None;
            };
            match resolve_type_file(path, cache, root, "documentation", "json") {
                Ok(path) => Some(path.to_string_lossy().into_owned()),
                Err(source) => {
                    warnings.push(format!(
                        "could not load documentation file {path:?}: {source}"
                    ));
                    None
                }
            }
        })
        .collect()
}

fn resolve_type_file(
    value: &str,
    cache: &Path,
    root: Option<&Path>,
    kind: &str,
    extension: &str,
) -> Result<PathBuf> {
    if value.starts_with("https://") || value.starts_with("http://") {
        let cached = cache
            .join("external")
            .join(format!("{kind}-{:016x}.{extension}", stable_hash(value)));
        return assets::external(value, &cached);
    }
    let path = expand_home(value);
    let resolved = if path.is_absolute() {
        path
    } else if let Some(root) = root {
        root.join(path)
    } else {
        std::env::current_dir()?.join(path)
    };
    if !resolved.is_file() {
        return Err(error(format!("{} is not a file", resolved.display())));
    }
    Ok(fs::canonicalize(resolved)?)
}

fn expand_home(value: &str) -> PathBuf {
    let Some(relative) = value
        .strip_prefix("~/")
        .or_else(|| value.strip_prefix("~\\"))
    else {
        return PathBuf::from(value);
    };
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map_or_else(
            || PathBuf::from(value),
            |home| PathBuf::from(home).join(relative),
        )
}

const fn stable_hash(value: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        hash ^= bytes[index] as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        index += 1;
    }
    hash
}

fn adapt_startup_arguments(
    arguments: &mut Vec<OsString>,
    settings: &Settings,
    cache: &Path,
    resources: Option<&assets::Paths>,
) {
    if !arguments
        .iter()
        .any(|argument| argument == OsStr::new("--stdio"))
    {
        arguments.push(OsString::from("--stdio"));
    }
    if let Some(resources) = resources {
        if !has_roblox_definition(arguments) {
            arguments.push(OsString::from(format!(
                "--definitions:@roblox={}",
                resources.definitions.display()
            )));
        }
        arguments.push(OsString::from(format!(
            "--docs={}",
            resources.documentation.display()
        )));
    }
    if !settings.boolean("luau-lsp.fflags.enableByDefault", false)
        && !arguments
            .iter()
            .any(|argument| argument == OsStr::new("--no-flags-enabled"))
    {
        arguments.push(OsString::from("--no-flags-enabled"));
    }
    if settings.boolean("luau-lsp.server.delayStartup", false)
        && !arguments
            .iter()
            .any(|argument| argument == OsStr::new("--delay-startup"))
    {
        arguments.push(OsString::from("--delay-startup"));
    }
    if settings.boolean("luau-lsp.server.crashReporting.enabled", false) {
        if !arguments
            .iter()
            .any(|argument| argument == OsStr::new("--enable-crash-reporting"))
        {
            arguments.push(OsString::from("--enable-crash-reporting"));
        }
        arguments.push(OsString::from(format!(
            "--crash-report-directory={}",
            cache.join("crash-reports").display()
        )));
    }
    if let Some(path) = settings.string("luau-lsp.server.baseLuaurc") {
        arguments.push(OsString::from(format!("--base-luaurc={path}")));
    }
}

fn take_settings_argument(arguments: &mut Vec<OsString>) -> Result<Option<PathBuf>> {
    let mut retained = Vec::with_capacity(arguments.len());
    let mut settings = None;
    let mut index = 0;
    while let Some(argument) = arguments.get(index) {
        let inline = argument.to_str().and_then(|value| {
            value
                .strip_prefix("--settings=")
                .or_else(|| value.strip_prefix("--settings:"))
        });
        if let Some(path) = inline {
            settings = Some(PathBuf::from(path));
        } else if argument == OsStr::new("--settings") {
            index += 1;
            let path = arguments
                .get(index)
                .ok_or_else(|| error("--settings requires a path"))?;
            settings = Some(PathBuf::from(path));
        } else {
            retained.push(argument.clone());
        }
        index += 1;
    }
    *arguments = retained;
    Ok(settings)
}

fn reject_pipe(arguments: &[OsString]) -> Result<()> {
    if arguments.iter().any(|argument| {
        argument == OsStr::new("--pipe")
            || argument
                .to_str()
                .is_some_and(|value| value.starts_with("--pipe=") || value.starts_with("--pipe:"))
    }) {
        return Err(error(
            "managed Roblox sessions require stdio; --pipe is only available with --platform standard",
        ));
    }
    Ok(())
}

fn has_roblox_definition(arguments: &[OsString]) -> bool {
    arguments.iter().any(|argument| {
        argument
            .to_str()
            .is_some_and(|value| value.contains("@roblox="))
    })
}

struct SessionFile {
    path: PathBuf,
}

impl SessionFile {
    fn new(cache: &Path, settings: &Value) -> Result<Self> {
        let directory = cache.join("sessions");
        fs::create_dir_all(&directory)?;
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)?
            .as_nanos();
        let path = directory.join(format!("{}-{nanos}.json", std::process::id()));
        fs::write(&path, serde_json::to_vec_pretty(settings)?)?;
        Ok(Self { path })
    }
}

impl Drop for SessionFile {
    fn drop(&mut self) {
        let _result = fs::remove_file(&self.path);
    }
}

fn exit_code(code: Option<i32>) -> u8 {
    code.and_then(|code| u8::try_from(code).ok()).unwrap_or(1)
}

fn write_stderr(message: &str) {
    let _result = writeln!(std::io::stderr().lock(), "luau-lsp: {message}");
}

#[cfg(test)]
mod tests {
    use super::{configured_definitions, take_settings_argument};
    use crate::{Result, error};
    use std::ffi::OsString;
    use std::fs;
    use std::path::Path;

    use serde_json::json;

    #[test]
    fn extracts_upstream_settings_for_normalization() -> Result<()> {
        let mut arguments = vec![
            OsString::from("lsp"),
            OsString::from("--settings=config.json"),
            OsString::from("--stdio"),
        ];
        let settings = take_settings_argument(&mut arguments)?;
        assert_eq!(settings.as_deref(), Some(Path::new("config.json")));
        assert_eq!(
            arguments,
            [OsString::from("lsp"), OsString::from("--stdio")]
        );
        Ok(())
    }

    #[test]
    fn resolves_relative_definition_files_from_the_workspace() -> Result<()> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)?
            .as_nanos();
        let workspace = std::env::temp_dir().join(format!(
            "luau-lsp-roblox-test-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&workspace)?;
        let definition = workspace.join("types.d.luau");
        fs::write(&definition, "declare global: string")?;
        let mut warnings = Vec::new();
        let resolved = configured_definitions(
            &json!({ "test": "types.d.luau" }),
            &workspace.join("cache"),
            Some(&workspace),
            &mut warnings,
        );
        let actual = resolved
            .get("test")
            .ok_or_else(|| error("test definition was not resolved"))?;
        assert_eq!(Path::new(actual), fs::canonicalize(&definition)?);
        assert!(warnings.is_empty());
        fs::remove_file(definition)?;
        fs::remove_dir(workspace)?;
        Ok(())
    }
}

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{BufRead, BufReader, BufWriter, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Map, Value};

use crate::args::{Args, Mode};
use crate::assets::{self, Security};
use crate::config::{Settings, normalize_change, normalize_response};
use crate::lsp::{self, Initialize};
use crate::process;
use crate::services;
use crate::{Result, error};

/// Runs the wrapper and returns its process exit code.
///
/// # Errors
///
/// Returns an error when arguments, configuration, resources, or a child process
/// cannot be prepared safely.
pub fn run() -> Result<u8> {
    if let Some(result) = process::run_guard() {
        return result;
    }
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
        match arguments.mode {
            Mode::Lsp => managed_lsp(arguments, &upstream),
            Mode::Analyze => managed_analyze(arguments, &upstream),
            Mode::Other => transparent(&arguments.forwarded, &upstream),
        }
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

fn managed_analyze(mut arguments: Args, upstream_path: &Path) -> Result<u8> {
    let (warnings, file) = prepare_analysis(&mut arguments, upstream_path)?;
    for warning in warnings {
        write_stderr(&format!("warning: {warning}"));
    }
    let result = transparent(&arguments.forwarded, upstream_path);
    drop(file);
    result
}

fn managed_lsp(mut arguments: Args, upstream_path: &Path) -> Result<u8> {
    reject_pipe(&arguments.forwarded)?;
    let mut input = BufReader::new(std::io::stdin());
    let first =
        lsp::read(&mut input)?.ok_or_else(|| error("LSP stdin closed before initialize"))?;
    let mut initialize = Initialize::parse(first)?;

    let PreparedSession {
        settings,
        warnings,
        file,
    } = prepare_session(&mut arguments, &mut initialize, upstream_path)?;

    let mut process = start_process(upstream_path, &arguments.forwarded, file)?;
    let client_done = Arc::new(AtomicBool::new(false));
    if let Err(source) = process.upstream.send(initialize.message) {
        terminate_process(&mut process);
        return Err(source.into());
    }
    for warning in warnings {
        let _result = process.client.send(lsp::log(2, warning));
    }

    let service_handles = match services::start(
        &settings,
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

    let client_reader = start_client_reader(input, &process, settings, Arc::clone(&client_done));

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
    let _result = process.server_stderr.join();
    drop(process.client);
    if normal_shutdown {
        let _result = process.client_writer.join();
    }
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
    server_stderr: thread::JoinHandle<()>,
}

fn terminate_process(process: &mut ProcessIo) {
    process.stop.store(true, Ordering::Release);
    if process.child.try_wait().ok().flatten().is_none() {
        let _result = process.child.kill();
        let _result = process.child.wait();
    }
}

fn start_process(
    upstream_path: &Path,
    arguments: &[OsString],
    session: SessionFile,
) -> Result<ProcessIo> {
    let mut child = Command::new(upstream_path)
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let child_stdin = child
        .stdin
        .take()
        .ok_or_else(|| error("failed to open bundled server stdin"))?;
    let child_stdout = child
        .stdout
        .take()
        .ok_or_else(|| error("failed to open bundled server stdout"))?;
    let child_stderr = child
        .stderr
        .take()
        .ok_or_else(|| error("failed to open bundled server stderr"))?;
    let stop = Arc::new(AtomicBool::new(false));
    let pending = Arc::new(Mutex::new(BTreeSet::<String>::new()));
    let (upstream, upstream_rx) = mpsc::channel::<Value>();
    let (client, client_rx) = mpsc::channel::<Value>();
    let upstream_writer = start_upstream_writer(child_stdin, upstream_rx);
    let client_writer = start_client_writer(client_rx, Arc::clone(&stop));
    let server_reader =
        start_server_reader(child_stdout, Arc::clone(&pending), client.clone(), session);
    let server_stderr = process::forward_stderr(child_stderr);
    Ok(ProcessIo {
        child,
        stop,
        upstream,
        client,
        pending,
        upstream_writer,
        client_writer,
        server_reader,
        server_stderr,
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
    session: SessionFile,
) -> thread::JoinHandle<()> {
    thread::spawn(move || read_server(BufReader::new(child_stdout), &pending, &client, session))
}

fn read_server<R: BufRead>(
    mut input: R,
    pending: &Mutex<BTreeSet<String>>,
    client: &mpsc::Sender<Value>,
    session: SessionFile,
) {
    let mut session = Some(session);
    loop {
        match lsp::read(&mut input) {
            Ok(Some(message)) => {
                drop(session.take());
                track_configuration_request(&message, pending);
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

fn prepare_analysis(
    arguments: &mut Args,
    upstream_path: &Path,
) -> Result<(Vec<String>, SessionFile)> {
    let mut settings = configured_settings(arguments, None)?;
    let cache = arguments.cache_path()?;
    fs::create_dir_all(&cache)?;
    let current = std::env::current_dir()?;
    let mut managed = Vec::new();
    let mut warnings = adapt_definition_files(&mut managed, &mut settings, &cache, Some(&current));

    if settings.boolean("luau-lsp.types.roblox", true)
        && !has_roblox_definition(&arguments.forwarded)
        && !has_roblox_definition(&managed)
    {
        let security = Security::parse(
            settings
                .string("luau-lsp.types.robloxSecurityLevel")
                .unwrap_or("PluginSecurity"),
        )?;
        let definitions = assets::prepare_definitions(upstream_path, &cache, security)?;
        managed.push(OsString::from(format!(
            "--definitions=@roblox={}",
            definitions.display()
        )));
    }

    let cli_flags = assets::take_cli_flags(&mut arguments.forwarded)?;
    let flags = assets::resolve_flags(upstream_path, &settings, None, cli_flags)?;
    managed.extend(
        flags
            .values
            .iter()
            .map(|(name, value)| OsString::from(format!("--flag:{name}={value}"))),
    );
    managed.push(OsString::from("--platform=roblox"));
    if !settings.boolean("luau-lsp.fflags.enableByDefault", false)
        && !has_option(&arguments.forwarded, "--no-flags-enabled")
    {
        managed.push(OsString::from("--no-flags-enabled"));
    }
    if let Some(path) = settings.string("luau-lsp.server.baseLuaurc")
        && !has_option(&arguments.forwarded, "--base-luaurc")
    {
        managed.push(OsString::from(format!("--base-luaurc={path}")));
    }

    if !has_option(&arguments.forwarded, "--sourcemap")
        && let Some(path) =
            services::analysis_sourcemap(&settings, &analyze_paths(&arguments.forwarded))?
    {
        managed.push(OsString::from(format!("--sourcemap={}", path.display())));
    }

    warnings.extend(settings.notices()?);
    warnings.extend(flags.warnings);
    settings.set("luau-lsp.fflags.enableByDefault", Value::Bool(true));
    settings.set("luau-lsp.fflags.sync", Value::Bool(false));
    let file = SessionFile::new(&cache, &settings.dotted())?;
    managed.push(OsString::from(format!(
        "--settings={}",
        file.path.display()
    )));
    arguments.forwarded.splice(1..1, managed);
    Ok((warnings, file))
}

fn prepare_session(
    arguments: &mut Args,
    initialize: &mut Initialize,
    upstream_path: &Path,
) -> Result<PreparedSession> {
    let mut settings = configured_settings(arguments, initialize.settings.as_ref())?;
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
    sync_definitions(&arguments.forwarded, &mut settings)?;
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

fn configured_settings(arguments: &mut Args, initialization: Option<&Value>) -> Result<Settings> {
    let mut settings = Settings::defaults();
    if let Some(path) = take_settings_argument(&mut arguments.forwarded)? {
        settings.merge(Settings::from_path(&path)?);
    }
    if let Some(path) = &arguments.settings {
        settings.merge(Settings::from_path(path)?);
    }
    if let Some(value) = initialization {
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
    Ok(settings)
}

fn adapt_type_files(
    arguments: &mut Vec<OsString>,
    settings: &mut Settings,
    cache: &Path,
    roots: &[PathBuf],
) -> Vec<String> {
    let root = roots.first().map(PathBuf::as_path);
    let mut warnings = adapt_definition_files(arguments, settings, cache, root);
    warnings.extend(adapt_documentation_files(arguments, settings, cache, root));
    warnings
}

fn adapt_definition_files(
    arguments: &mut Vec<OsString>,
    settings: &mut Settings,
    cache: &Path,
    root: Option<&Path>,
) -> Vec<String> {
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
    warnings
}

fn sync_definitions(arguments: &[OsString], settings: &mut Settings) -> Result<()> {
    let mut definitions = Map::new();
    let mut unnamed = 0;
    let mut index = 0;
    while let Some(argument) = arguments.get(index) {
        let inline = argument.to_str().and_then(|value| {
            value
                .strip_prefix("--definitions:")
                .or_else(|| value.strip_prefix("--definitions="))
        });
        let definition = if let Some(definition) = inline {
            Some(definition)
        } else if argument == OsStr::new("--definitions") {
            index += 1;
            Some(
                arguments
                    .get(index)
                    .and_then(|value| value.to_str())
                    .ok_or_else(|| error("--definitions requires a UTF-8 definition path"))?,
            )
        } else {
            None
        };
        if let Some(definition) = definition {
            let (package, path) = definition.split_once('=').map_or_else(
                || {
                    let package = if unnamed == 0 {
                        "@roblox".to_owned()
                    } else {
                        format!("@roblox{unnamed}")
                    };
                    unnamed += 1;
                    (package, definition)
                },
                |(package, path)| {
                    let package = if package.starts_with('@') {
                        package.to_owned()
                    } else {
                        format!("@{package}")
                    };
                    (package, path)
                },
            );
            definitions
                .entry(package)
                .or_insert_with(|| Value::String(path.to_owned()));
        }
        index += 1;
    }
    settings.set("luau-lsp.types.definitionFiles", Value::Object(definitions));
    Ok(())
}

fn adapt_documentation_files(
    arguments: &mut Vec<OsString>,
    settings: &mut Settings,
    cache: &Path,
    root: Option<&Path>,
) -> Vec<String> {
    let mut warnings = Vec::new();
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
    Ok(normalize_definition_path(fs::canonicalize(resolved)?))
}

fn normalize_definition_path(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let Some(path) = path.to_str() else {
            return path;
        };
        if let Some(path) = path.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{path}"));
        }
        if let Some(path) = path.strip_prefix(r"\\?\") {
            return PathBuf::from(path);
        }
    }
    path
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

const ANALYZE_VALUE_OPTIONS: [&str; 9] = [
    "--flag",
    "--formatter",
    "--sourcemap",
    "--definitions",
    "--defs",
    "--ignore",
    "--base-luaurc",
    "--platform",
    "--settings",
];

fn analyze_paths(arguments: &[OsString]) -> Vec<PathBuf> {
    let mut index = 1;
    while let Some(argument) = arguments.get(index) {
        if argument == OsStr::new("--") {
            return arguments[index + 1..].iter().map(PathBuf::from).collect();
        }
        let Some(value) = argument.to_str() else {
            return arguments[index..].iter().map(PathBuf::from).collect();
        };
        if !value.starts_with('-') {
            return arguments[index..].iter().map(PathBuf::from).collect();
        }
        let (name, inline) = value
            .split_once([':', '='])
            .map_or((value, false), |(name, _value)| (name, true));
        index += usize::from(
            !inline && ANALYZE_VALUE_OPTIONS.contains(&name) && arguments.get(index + 1).is_some(),
        ) + 1;
    }
    Vec::new()
}

fn has_option(arguments: &[OsString], name: &str) -> bool {
    arguments.iter().any(|argument| {
        if argument == OsStr::new(name) {
            return true;
        }
        argument
            .to_str()
            .and_then(|value| value.strip_prefix(name))
            .is_some_and(|suffix| suffix.starts_with([':', '=']))
    })
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

const STALE_SESSION_AGE: Duration = Duration::from_secs(10 * 60);

impl SessionFile {
    fn new(cache: &Path, settings: &Value) -> Result<Self> {
        let directory = cache.join("sessions");
        fs::create_dir_all(&directory)?;
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)?
            .as_nanos();
        remove_stale_sessions(&directory, nanos);
        let path = directory.join(format!("{}-{nanos}.json", std::process::id()));
        fs::write(&path, serde_json::to_vec_pretty(settings)?)?;
        Ok(Self { path })
    }
}

fn remove_stale_sessions(directory: &Path, now: u128) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !entry.file_type().is_ok_and(|kind| kind.is_file())
            || path.extension() != Some(OsStr::new("json"))
        {
            continue;
        }
        let Some((pid, created)) = path
            .file_stem()
            .and_then(OsStr::to_str)
            .and_then(|stem| stem.split_once('-'))
        else {
            continue;
        };
        let (Ok(_pid), Ok(created)) = (pid.parse::<u32>(), created.parse::<u128>()) else {
            continue;
        };
        if now.saturating_sub(created) >= STALE_SESSION_AGE.as_nanos() {
            let _result = fs::remove_file(path);
        }
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
    use super::{
        STALE_SESSION_AGE, SessionFile, adapt_startup_arguments, analyze_paths,
        configured_definitions, normalize_definition_path, read_server, remove_stale_sessions,
        sync_definitions, take_settings_argument,
    };
    use crate::{Result, error};
    use std::collections::BTreeSet;
    use std::ffi::OsString;
    use std::fs;
    use std::io::BufReader;
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, mpsc};

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
    fn extracts_analysis_paths_after_upstream_options() {
        let arguments = [
            "analyze",
            "--formatter",
            "gnu",
            "--definitions:@roblox=types.d.luau",
            "--ignore=vendor/**",
            "places/earth/src",
            "places/earth/tests/main.luau",
        ]
        .map(OsString::from);

        assert_eq!(
            analyze_paths(&arguments),
            [
                PathBuf::from("places/earth/src"),
                PathBuf::from("places/earth/tests/main.luau")
            ]
        );
    }

    #[test]
    fn mirrors_loaded_definitions_into_settings() -> Result<()> {
        let arguments = [
            "lsp",
            "--definitions:@roblox=C:/cache/globalTypes.d.luau",
            "--definitions",
            "game=C:/workspace/game.d.luau",
        ]
        .map(OsString::from);
        let mut settings = crate::config::Settings::defaults();

        sync_definitions(&arguments, &mut settings)?;

        assert_eq!(
            settings.get("luau-lsp.types.definitionFiles"),
            Some(&json!({
                "@roblox": "C:/cache/globalTypes.d.luau",
                "@game": "C:/workspace/game.d.luau"
            }))
        );
        Ok(())
    }

    #[test]
    fn managed_definition_uses_the_same_cli_and_settings_path() -> Result<()> {
        let definition = PathBuf::from("C:/cache/globalTypes.PluginSecurity.d.luau");
        let resources = crate::assets::Paths {
            definitions: definition.clone(),
            documentation: PathBuf::from("C:/cache/api-docs.json"),
        };
        let mut arguments = vec![OsString::from("lsp")];
        let mut settings = crate::config::Settings::defaults();

        adapt_startup_arguments(
            &mut arguments,
            &settings,
            Path::new("C:/cache"),
            Some(&resources),
        );
        sync_definitions(&arguments, &mut settings)?;

        assert!(arguments.contains(&OsString::from(format!(
            "--definitions:@roblox={}",
            definition.display()
        ))));
        assert_eq!(
            settings
                .get("luau-lsp.types.definitionFiles")
                .and_then(|value| value.get("@roblox")),
            Some(&json!(definition.to_string_lossy()))
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
        assert_eq!(
            Path::new(actual),
            normalize_definition_path(fs::canonicalize(&definition)?)
        );
        assert!(warnings.is_empty());
        fs::remove_file(definition)?;
        fs::remove_dir(workspace)?;
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn removes_windows_verbatim_prefix_from_definition_files() {
        assert_eq!(
            normalize_definition_path(PathBuf::from(r"\\?\C:\workspace\types.d.luau")),
            PathBuf::from(r"C:\workspace\types.d.luau")
        );
        assert_eq!(
            normalize_definition_path(PathBuf::from(r"\\?\UNC\server\share\types.d.luau")),
            PathBuf::from(r"\\server\share\types.d.luau")
        );
    }

    #[test]
    fn removes_session_file_after_upstream_starts() -> Result<()> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)?
            .as_nanos();
        let workspace = std::env::temp_dir().join(format!(
            "luau-lsp-roblox-session-test-{}-{nanos}",
            std::process::id()
        ));
        let session = SessionFile::new(&workspace, &json!({ "test": true }))?;
        let path = session.path.clone();
        let message = json!({ "jsonrpc": "2.0", "id": 0, "result": {} });
        let mut framed = Vec::new();
        crate::lsp::write(&mut framed, &message)?;
        let (client, received) = mpsc::channel();

        read_server(
            BufReader::new(framed.as_slice()),
            &Mutex::new(BTreeSet::new()),
            &client,
            session,
        );

        assert_eq!(received.recv()?, message);
        assert!(!path.exists());
        fs::remove_dir_all(workspace)?;
        Ok(())
    }

    #[test]
    fn removes_only_stale_session_files() -> Result<()> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)?
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "luau-lsp-roblox-stale-test-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&directory)?;
        let now = STALE_SESSION_AGE.as_nanos() + 2;
        let stale = directory.join("123-1.json");
        let current = directory.join(format!("456-{now}.json"));
        let unrelated = directory.join("notes.json");
        fs::write(&stale, "{}")?;
        fs::write(&current, "{}")?;
        fs::write(&unrelated, "{}")?;

        remove_stale_sessions(&directory, now);

        assert!(!stale.exists());
        assert!(current.exists());
        assert!(unrelated.exists());
        fs::remove_dir_all(directory)?;
        Ok(())
    }
}

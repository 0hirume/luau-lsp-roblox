use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher as _};
use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};

use crate::config::Settings;
use crate::lsp;
use crate::{Result, error};

const POLL_INTERVAL: Duration = Duration::from_millis(300);
const HTTP_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_HEADER_SIZE: usize = 16 * 1024;

pub fn start(
    settings: &Settings,
    roots: &[PathBuf],
    upstream: &mpsc::Sender<Value>,
    client: &mpsc::Sender<Value>,
    stop: &Arc<AtomicBool>,
) -> Result<Vec<JoinHandle<()>>> {
    let mut handles = Vec::new();
    if studio_enabled(settings) {
        handles.push(start_studio(
            settings,
            roots.to_vec(),
            upstream.clone(),
            client.clone(),
            Arc::clone(stop),
        )?);
    }
    handles.extend(start_sourcemaps(settings, roots, upstream, client, stop));
    Ok(handles)
}

fn start_sourcemaps(
    settings: &Settings,
    roots: &[PathBuf],
    upstream: &mpsc::Sender<Value>,
    client: &mpsc::Sender<Value>,
    stop: &Arc<AtomicBool>,
) -> Vec<JoinHandle<()>> {
    if !settings.boolean("luau-lsp.sourcemap.enabled", true) {
        return Vec::new();
    }
    let options = Sourcemap::from_settings(settings);
    let mut handles = Vec::new();
    for root in roots {
        let path = resolve(root, &options.file);
        let watch_path = path.clone();
        let watch_tx = upstream.clone();
        let watch_stop = Arc::clone(stop);
        handles.push(thread::spawn(move || {
            monitor_sourcemap(&watch_path, &watch_tx, &watch_stop);
        }));

        if options.autogenerate {
            let Some(project) = find_project(root, &options.project) else {
                let _result = client.send(lsp::log(
                    2,
                    format!(
                        "unable to find Rojo project {} under {}; sourcemap generation is disabled for this workspace",
                        options.project,
                        root.display()
                    ),
                ));
                continue;
            };
            let generate_options = options.clone();
            let generate_root = root.clone();
            let generate_tx = client.clone();
            let generate_stop = Arc::clone(stop);
            handles.push(thread::spawn(move || {
                generate(
                    &generate_options,
                    &generate_root,
                    &project,
                    &generate_tx,
                    &generate_stop,
                );
            }));
        }
    }
    handles
}

#[derive(Clone, Debug)]
struct Sourcemap {
    autogenerate: bool,
    rojo: String,
    project: String,
    file: String,
    include_non_scripts: bool,
    generator: Option<String>,
    wrapper_watcher: bool,
}

impl Sourcemap {
    fn from_settings(settings: &Settings) -> Self {
        Self {
            autogenerate: settings.boolean("luau-lsp.sourcemap.autogenerate", true),
            rojo: settings
                .string("luau-lsp.sourcemap.rojoPath")
                .unwrap_or("rojo")
                .to_owned(),
            project: settings
                .string("luau-lsp.sourcemap.rojoProjectFile")
                .unwrap_or("default.project.json")
                .to_owned(),
            file: settings
                .string("luau-lsp.sourcemap.sourcemapFile")
                .unwrap_or("sourcemap.json")
                .to_owned(),
            include_non_scripts: settings.boolean("luau-lsp.sourcemap.includeNonScripts", true),
            generator: settings
                .string("luau-lsp.sourcemap.generatorCommand")
                .map(str::to_owned),
            wrapper_watcher: settings.boolean("luau-lsp.sourcemap.useVSCodeWatcher", false),
        }
    }
}

fn generate(
    options: &Sourcemap,
    root: &Path,
    project: &Path,
    client: &mpsc::Sender<Value>,
    stop: &AtomicBool,
) {
    if options.wrapper_watcher {
        let mut signature = None;
        while !stop.load(Ordering::Acquire) {
            let current = workspace_signature(root);
            if signature != Some(current) {
                signature = Some(current);
                let command = generator_command(options, root, project, false);
                if !run_generator(&command, root, client, stop) {
                    return;
                }
            }
            sleep(stop, POLL_INTERVAL);
        }
    } else {
        let command = generator_command(options, root, project, true);
        let completed = run_generator(&command, root, client, stop);
        if completed && !stop.load(Ordering::Acquire) {
            let _result = client.send(lsp::log(
                2,
                format!(
                    "sourcemap generator for {} exited; updates are no longer being generated",
                    root.display()
                ),
            ));
        }
    }
}

fn generator_command(options: &Sourcemap, root: &Path, project: &Path, watch: bool) -> Vec<String> {
    if let Some(command) = &options.generator {
        return split_command(command);
    }
    let project = project
        .strip_prefix(root)
        .unwrap_or(project)
        .to_string_lossy()
        .into_owned();
    let mut command = vec![
        options.rojo.clone(),
        "sourcemap".to_owned(),
        project,
        "--output".to_owned(),
        options.file.clone(),
    ];
    if options.include_non_scripts {
        command.push("--include-non-scripts".to_owned());
    }
    if watch {
        command.push("--watch".to_owned());
    }
    command
}

fn run_generator(
    command: &[String],
    root: &Path,
    client: &mpsc::Sender<Value>,
    stop: &AtomicBool,
) -> bool {
    let Some((program, arguments)) = command.split_first() else {
        let _result = client.send(lsp::log(1, "sourcemap generator command is empty"));
        return false;
    };
    let mut child = match Command::new(program)
        .args(arguments)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Ok(child) => child,
        Err(source) => {
            let _result = client.send(lsp::log(
                1,
                format!("failed to start sourcemap generator {program:?}: {source}"),
            ));
            return false;
        }
    };

    while !stop.load(Ordering::Acquire) {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    let _result = client.send(lsp::log(
                        1,
                        format!("sourcemap generator exited with {status}"),
                    ));
                }
                return status.success();
            }
            Ok(None) => sleep(stop, Duration::from_millis(50)),
            Err(source) => {
                let _result = client.send(lsp::log(
                    1,
                    format!("failed to inspect sourcemap generator: {source}"),
                ));
                return false;
            }
        }
    }
    let _result = child.kill();
    let _result = child.wait();
    false
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Stamp {
    exists: bool,
    modified: Option<SystemTime>,
    length: u64,
}

fn monitor_sourcemap(path: &Path, upstream: &mpsc::Sender<Value>, stop: &AtomicBool) {
    let mut stamp = file_stamp(path);
    while !stop.load(Ordering::Acquire) {
        sleep(stop, POLL_INTERVAL);
        let current = file_stamp(path);
        if current != stamp {
            stamp = current;
            if upstream.send(lsp::sourcemap_changed(path)).is_err() {
                return;
            }
        }
    }
}

fn file_stamp(path: &Path) -> Stamp {
    match fs::metadata(path) {
        Ok(metadata) => Stamp {
            exists: true,
            modified: metadata.modified().ok(),
            length: metadata.len(),
        },
        Err(_source) => Stamp {
            exists: false,
            modified: None,
            length: 0,
        },
    }
}

fn workspace_signature(root: &Path) -> u64 {
    let mut hasher = DefaultHasher::new();
    hash_workspace(root, &mut hasher);
    hasher.finish()
}

fn hash_workspace(path: &Path, hasher: &mut DefaultHasher) {
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    let mut entries: Vec<_> = entries.filter_map(std::result::Result::ok).collect();
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            if !ignored_directory(&path) {
                hash_workspace(&path, hasher);
            }
        } else if is_source(&path) || is_project(&path) {
            path.hash(hasher);
            if let Ok(metadata) = entry.metadata() {
                metadata.len().hash(hasher);
                metadata.modified().ok().hash(hasher);
            }
        }
    }
}

fn find_project(root: &Path, configured: &str) -> Option<PathBuf> {
    let configured = resolve(root, configured);
    if configured.is_file() {
        return Some(configured);
    }
    let entries = fs::read_dir(root).ok()?;
    let mut candidates: Vec<_> = entries
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| is_project(path))
        .collect();
    candidates.sort();
    (candidates.len() == 1).then(|| candidates.remove(0))
}

fn resolve(root: &Path, path: &str) -> PathBuf {
    let path = PathBuf::from(path);
    if path.is_absolute() {
        path
    } else {
        root.join(path)
    }
}

fn is_project(path: &Path) -> bool {
    path.file_name()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|name| name.ends_with(".project.json"))
}

fn is_source(path: &Path) -> bool {
    matches!(
        path.extension().and_then(std::ffi::OsStr::to_str),
        Some("lua" | "luau")
    )
}

fn ignored_directory(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(std::ffi::OsStr::to_str),
        Some(".git" | ".hg" | ".svn" | "node_modules")
    )
}

fn split_command(input: &str) -> Vec<String> {
    let mut arguments = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;
    for character in input.chars() {
        if escaped {
            current.push(character);
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if quote == Some(character) {
            quote = None;
        } else if quote.is_none() && matches!(character, '\'' | '"') {
            quote = Some(character);
        } else if quote.is_none() && character.is_whitespace() {
            if !current.is_empty() {
                arguments.push(std::mem::take(&mut current));
            }
        } else {
            current.push(character);
        }
    }
    if escaped {
        current.push('\\');
    }
    if !current.is_empty() {
        arguments.push(current);
    }
    arguments
}

fn studio_enabled(settings: &Settings) -> bool {
    settings.boolean("luau-lsp.studioPlugin.enabled", false)
        || settings.boolean("luau-lsp.plugin.enabled", false)
}

fn start_studio(
    settings: &Settings,
    roots: Vec<PathBuf>,
    upstream: mpsc::Sender<Value>,
    client: mpsc::Sender<Value>,
    stop: Arc<AtomicBool>,
) -> Result<JoinHandle<()>> {
    let port = settings
        .integer("luau-lsp.studioPlugin.port")
        .or_else(|| settings.integer("luau-lsp.plugin.port"))
        .unwrap_or(3667);
    let port = u16::try_from(port).map_err(|_source| error("Studio port must fit in 16 bits"))?;
    let limit = settings
        .string("luau-lsp.studioPlugin.maximumRequestBodySize")
        .or_else(|| settings.string("luau-lsp.plugin.maximumRequestBodySize"))
        .map_or(Ok(3 * 1024 * 1024), parse_size)?;
    let listener = TcpListener::bind(("127.0.0.1", port)).map_err(|source| {
        error(format!(
            "failed to bind Studio companion on 127.0.0.1:{port}: {source}"
        ))
    })?;
    listener.set_nonblocking(true)?;

    Ok(thread::spawn(move || {
        let _result = client.send(lsp::log(
            3,
            format!("Studio companion listening on 127.0.0.1:{port}"),
        ));
        while !stop.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((stream, _address)) => {
                    if let Err(source) = handle_http(stream, limit, &roots, &upstream) {
                        let _result = client.send(lsp::log(
                            1,
                            format!("Studio companion request failed: {source}"),
                        ));
                    }
                }
                Err(source) if source.kind() == std::io::ErrorKind::WouldBlock => {
                    sleep(&stop, Duration::from_millis(50));
                }
                Err(source) => {
                    let _result = client.send(lsp::log(
                        1,
                        format!("Studio companion listener failed: {source}"),
                    ));
                    return;
                }
            }
        }
    }))
}

#[derive(Debug)]
struct HttpRequest {
    method: String,
    target: String,
    body: Vec<u8>,
}

fn handle_http(
    mut stream: TcpStream,
    limit: usize,
    roots: &[PathBuf],
    upstream: &mpsc::Sender<Value>,
) -> Result<()> {
    stream.set_read_timeout(Some(HTTP_TIMEOUT))?;
    stream.set_write_timeout(Some(HTTP_TIMEOUT))?;
    let request = match read_http(&mut stream, limit) {
        Ok(request) => request,
        Err(source) => {
            respond(
                &mut stream,
                400,
                "text/plain",
                source.to_string().as_bytes(),
            )?;
            return Ok(());
        }
    };

    match (request.method.as_str(), request.target.as_str()) {
        ("POST", "/full") => {
            let body: Value = serde_json::from_slice(&request.body)?;
            let Some(tree) = body.get("tree").cloned() else {
                respond(&mut stream, 400, "text/plain", b"missing tree")?;
                return Ok(());
            };
            upstream.send(lsp::plugin_full(&tree))?;
            respond(&mut stream, 200, "text/plain", b"OK")?;
        }
        ("POST", "/clear") => {
            upstream.send(lsp::plugin_clear())?;
            respond(&mut stream, 200, "text/plain", b"OK")?;
        }
        ("GET", "/get-file-paths") => {
            let mut files = Vec::new();
            for root in roots {
                collect_sources(root, &mut files);
            }
            files.sort();
            files.dedup();
            let body = serde_json::to_vec(&json!({ "files": files }))?;
            respond(&mut stream, 200, "application/json", &body)?;
        }
        _ => respond(&mut stream, 404, "text/plain", b"Not Found")?,
    }
    Ok(())
}

fn read_http(stream: &mut TcpStream, limit: usize) -> Result<HttpRequest> {
    let mut received = Vec::new();
    let header_end = loop {
        if received.len() >= MAX_HEADER_SIZE {
            return Err(error("HTTP headers exceed the size limit"));
        }
        let mut block = [0_u8; 1024];
        let bytes = stream.read(&mut block)?;
        if bytes == 0 {
            return Err(error("unexpected EOF in HTTP headers"));
        }
        received.extend_from_slice(&block[..bytes]);
        if let Some(position) = received.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
    };

    let headers = std::str::from_utf8(&received[..header_end])?;
    let mut lines = headers.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| error("missing HTTP request line"))?;
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts
        .next()
        .ok_or_else(|| error("missing HTTP method"))?
        .to_owned();
    let target = request_parts
        .next()
        .ok_or_else(|| error("missing HTTP target"))?
        .to_owned();
    let mut content_length = 0_usize;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse()?;
            }
        }
    }
    if content_length > limit {
        return Err(error(format!("request body exceeds {limit} bytes")));
    }

    let mut body = received[header_end..].to_vec();
    if body.len() > content_length {
        body.truncate(content_length);
    } else if body.len() < content_length {
        let missing = content_length - body.len();
        let mut remainder = vec![0_u8; missing];
        stream.read_exact(&mut remainder)?;
        body.extend(remainder);
    }
    Ok(HttpRequest {
        method,
        target,
        body,
    })
}

fn respond(stream: &mut TcpStream, status: u16, content_type: &str, body: &[u8]) -> Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        _ => "Error",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    stream.flush()?;
    Ok(())
}

fn collect_sources(path: &Path, output: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    for entry in entries.filter_map(std::result::Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            if !ignored_directory(&path) {
                collect_sources(&path, output);
            }
        } else if is_source(&path) {
            output.push(path.to_string_lossy().into_owned());
        }
    }
}

fn parse_size(input: &str) -> Result<usize> {
    let input = input.trim().to_ascii_lowercase();
    let digit_end = input
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(input.len());
    let number: usize = input[..digit_end].parse()?;
    let multiplier = match input[digit_end..].trim() {
        "" | "b" => 1,
        "kb" => 1000,
        "kib" => 1024,
        "mb" => 1000 * 1000,
        "mib" => 1024 * 1024,
        "gb" => 1000 * 1000 * 1000,
        "gib" => 1024 * 1024 * 1024,
        suffix => return Err(error(format!("unsupported byte-size suffix {suffix:?}"))),
    };
    number
        .checked_mul(multiplier)
        .ok_or_else(|| error("request body size overflows usize"))
}

fn sleep(stop: &AtomicBool, duration: Duration) {
    let deadline = std::time::Instant::now() + duration;
    while !stop.load(Ordering::Acquire) && std::time::Instant::now() < deadline {
        thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_size, split_command};
    use crate::Result;

    #[test]
    fn parses_generator_commands_without_a_shell() {
        assert_eq!(
            split_command("rojo sourcemap 'game project.json' --watch"),
            ["rojo", "sourcemap", "game project.json", "--watch"]
        );
    }

    #[test]
    fn parses_studio_body_limits() -> Result<()> {
        assert_eq!(parse_size("3mb")?, 3_000_000);
        assert_eq!(parse_size("4 MiB")?, 4 * 1024 * 1024);
        assert!(parse_size("one megabyte").is_err());
        Ok(())
    }
}

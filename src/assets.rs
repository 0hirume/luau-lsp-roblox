use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::args::sibling_resource;
use crate::config::Settings;
use crate::{Result, error};

const UPSTREAM_JSON: &str = include_str!("../upstream/manifest.json");
const MAX_DOWNLOAD_SIZE: u64 = 64 * 1024 * 1024;
const REFRESH_AGE: Duration = Duration::from_secs(24 * 60 * 60);
const FLAG_KINDS: [&str; 4] = ["FFlag", "FInt", "DFFlag", "DFInt"];

#[derive(Clone, Debug, Deserialize)]
pub struct Upstream {
    pub version: String,
    pub commit: String,
    roblox: Roblox,
}

#[derive(Clone, Debug, Deserialize)]
struct Roblox {
    definitions: String,
    documentation: String,
    fflags: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Security {
    None,
    LocalUser,
    Plugin,
    RobloxScript,
}

impl Security {
    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "None" => Ok(Self::None),
            "LocalUserSecurity" => Ok(Self::LocalUser),
            "PluginSecurity" => Ok(Self::Plugin),
            "RobloxScriptSecurity" => Ok(Self::RobloxScript),
            _ => Err(error(format!(
                "invalid Roblox security level {value:?}; expected None, LocalUserSecurity, PluginSecurity, or RobloxScriptSecurity"
            ))),
        }
    }

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::LocalUser => "LocalUserSecurity",
            Self::Plugin => "PluginSecurity",
            Self::RobloxScript => "RobloxScriptSecurity",
        }
    }
}

#[derive(Debug)]
pub struct Paths {
    pub definitions: PathBuf,
    pub documentation: PathBuf,
}

#[derive(Debug, Default)]
pub struct Flags {
    pub values: BTreeMap<String, String>,
    pub warnings: Vec<String>,
}

#[derive(Debug)]
enum Fetch {
    Modified {
        contents: Vec<u8>,
        etag: Option<String>,
    },
    NotModified,
}

pub fn upstream() -> Result<Upstream> {
    serde_json::from_str(UPSTREAM_JSON)
        .map_err(|source| error(format!("invalid embedded upstream manifest: {source}")))
}

pub fn prepare(upstream_path: &Path, cache_root: &Path, security: Security) -> Result<Paths> {
    let manifest = upstream()?;
    let cache = cache_root.join(&manifest.version);
    fs::create_dir_all(&cache)?;
    let bundled = sibling_resource(upstream_path)?;

    let definitions = prepare_definitions_from(&manifest, &cache, &bundled, security)?;
    let documentation = ensure(
        &manifest.roblox.documentation,
        &cache.join("api-docs.json"),
        &bundled.join("api-docs.json"),
    )?;

    Ok(Paths {
        definitions,
        documentation,
    })
}

pub fn prepare_definitions(
    upstream_path: &Path,
    cache_root: &Path,
    security: Security,
) -> Result<PathBuf> {
    let manifest = upstream()?;
    let cache = cache_root.join(&manifest.version);
    fs::create_dir_all(&cache)?;
    let bundled = sibling_resource(upstream_path)?;
    prepare_definitions_from(&manifest, &cache, &bundled, security)
}

fn prepare_definitions_from(
    manifest: &Upstream,
    cache: &Path,
    bundled: &Path,
    security: Security,
) -> Result<PathBuf> {
    let name = format!("globalTypes.{}.d.luau", security.name());
    let url = format!("{}{}.d.luau", manifest.roblox.definitions, security.name());
    ensure(&url, &cache.join(&name), &bundled.join(name))
}

/// Downloads or refreshes a configured external type resource.
///
/// # Errors
///
/// Returns an error for unsupported URLs, failed uncached downloads, or cache
/// writes that cannot be completed.
pub fn external(url: &str, cached: &Path) -> Result<PathBuf> {
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return Err(error(format!("unsupported external file URL {url:?}")));
    }
    if !stale(cached) {
        return Ok(cached.to_path_buf());
    }
    match download(url) {
        Ok(contents) => {
            atomic_write(cached, &contents)?;
            Ok(cached.to_path_buf())
        }
        Err(source) if cached.is_file() => {
            write_warning(&format!(
                "could not refresh {}; using cached data: {source}",
                cached.display()
            ));
            Ok(cached.to_path_buf())
        }
        Err(source) => Err(error(format!(
            "could not obtain external file from {url}: {source}"
        ))),
    }
}

pub fn take_cli_flags(arguments: &mut Vec<OsString>) -> Result<BTreeMap<String, String>> {
    let mut flags = BTreeMap::new();
    let mut retained = Vec::with_capacity(arguments.len());
    let mut index = 0;

    while let Some(argument) = arguments.get(index) {
        let text = argument.to_str();
        let inline = text.and_then(|value| {
            value
                .strip_prefix("--flag:")
                .or_else(|| value.strip_prefix("--flag="))
        });
        if let Some(pair) = inline {
            let (name, value) = parse_pair(pair)?;
            flags.insert(name, value);
        } else if argument == OsStr::new("--flag") {
            index += 1;
            let pair = arguments
                .get(index)
                .and_then(|value| value.to_str())
                .ok_or_else(|| error("--flag requires a UTF-8 KEY=VALUE argument"))?;
            let (name, value) = parse_pair(pair)?;
            flags.insert(name, value);
        } else {
            retained.push(argument.clone());
        }
        index += 1;
    }

    *arguments = retained;
    Ok(flags)
}

pub fn resolve_flags(
    upstream_path: &Path,
    settings: &Settings,
    initialization: Option<&Map<String, Value>>,
    cli: BTreeMap<String, String>,
) -> Result<Flags> {
    let supported = supported_flags(upstream_path)?;
    let mut output = Flags::default();

    if settings.boolean("luau-lsp.fflags.sync", true) {
        match scraped_flags() {
            Ok(scraped) => {
                for (name, value) in scraped {
                    if supported.contains_key(&name) {
                        output.values.insert(name, value);
                    }
                }
            }
            Err(source) => output.warnings.push(format!(
                "failed to synchronize Roblox FFlags; bundled server values remain active: {source}"
            )),
        }
    }

    if settings.boolean("luau-lsp.fflags.enableNewSolver", false) {
        apply_flag(
            &mut output,
            &supported,
            "LuauSolverV2".to_owned(),
            "true".to_owned(),
            "fflags.enableNewSolver",
        );
    }

    if let Some(overrides) = settings.object("luau-lsp.fflags.override") {
        for (name, value) in overrides {
            apply_flag(
                &mut output,
                &supported,
                normalize_flag_name(name),
                value_string(value),
                "fflags.override",
            );
        }
    }

    if let Some(initialization) = initialization {
        for (name, value) in initialization {
            apply_flag(
                &mut output,
                &supported,
                normalize_flag_name(name),
                value_string(value),
                "initializationOptions.fflags",
            );
        }
    }

    for (name, value) in cli {
        apply_flag(
            &mut output,
            &supported,
            normalize_flag_name(&name),
            value,
            "--flag",
        );
    }

    Ok(output)
}

fn ensure(url: &str, cached: &Path, bundled: &Path) -> Result<PathBuf> {
    if !needs_revalidation(cached) {
        return Ok(cached.to_path_buf());
    }

    match revalidate(url, cached) {
        Ok(()) => Ok(cached.to_path_buf()),
        Err(source) if cached.is_file() => {
            write_warning(&format!(
                "could not refresh {}; using cached data: {source}",
                cached.display()
            ));
            Ok(cached.to_path_buf())
        }
        Err(source) if bundled.is_file() => {
            write_warning(&format!(
                "could not download {url}; using packaged data: {source}"
            ));
            Ok(bundled.to_path_buf())
        }
        Err(source) => Err(error(format!(
            "could not obtain required Roblox data from {url}, and no packaged fallback exists at {}: {source}",
            bundled.display()
        ))),
    }
}

fn needs_revalidation(cached: &Path) -> bool {
    if !cached.is_file() {
        return true;
    }

    let etag = etag_path(cached);
    let stamp = if etag.is_file() {
        etag.as_path()
    } else {
        cached
    };
    stale(stamp)
}

fn stale(stamp: &Path) -> bool {
    let Ok(metadata) = fs::metadata(stamp) else {
        return true;
    };
    let Ok(modified) = metadata.modified() else {
        return true;
    };
    SystemTime::now()
        .duration_since(modified)
        .is_ok_and(|age| age >= REFRESH_AGE)
}

fn revalidate(url: &str, cached: &Path) -> Result<()> {
    revalidate_with(url, cached, fetch)
}

fn revalidate_with(
    url: &str,
    cached: &Path,
    fetch_resource: impl FnOnce(&str, Option<&str>) -> Result<Fetch>,
) -> Result<()> {
    let etag = cached_etag(cached);
    match fetch_resource(url, etag.as_deref())? {
        Fetch::Modified { contents, etag } => {
            atomic_write(cached, &contents)?;
            write_etag(cached, etag.as_deref())
        }
        Fetch::NotModified => {
            let etag = etag.ok_or_else(|| {
                error(format!(
                    "GET {url} returned 304 without a cached ETag validator"
                ))
            })?;
            if !cached.is_file() {
                return Err(error(format!(
                    "GET {url} returned 304 without a cached resource"
                )));
            }
            write_etag(cached, Some(&etag))
        }
    }
}

fn fetch(url: &str, etag: Option<&str>) -> Result<Fetch> {
    let mut request = ureq::get(url).set(
        "User-Agent",
        concat!("luau-lsp-roblox/", env!("CARGO_PKG_VERSION")),
    );
    if let Some(etag) = etag {
        request = request.set("If-None-Match", etag);
    }
    let response = request
        .call()
        .map_err(|source| error(format!("GET {url} failed: {source}")))?;
    if response.status() == 304 {
        return Ok(Fetch::NotModified);
    }
    if !(200..300).contains(&response.status()) {
        return Err(error(format!(
            "GET {url} returned HTTP {}",
            response.status()
        )));
    }

    let etag = response
        .header("ETag")
        .filter(|value| valid_etag(value))
        .map(str::to_owned);
    let mut contents = Vec::new();
    response
        .into_reader()
        .take(MAX_DOWNLOAD_SIZE + 1)
        .read_to_end(&mut contents)?;
    if u64::try_from(contents.len())? > MAX_DOWNLOAD_SIZE {
        return Err(error(format!(
            "GET {url} exceeded {MAX_DOWNLOAD_SIZE} bytes"
        )));
    }
    Ok(Fetch::Modified { contents, etag })
}

fn download(url: &str) -> Result<Vec<u8>> {
    match fetch(url, None)? {
        Fetch::Modified { contents, .. } => Ok(contents),
        Fetch::NotModified => Err(error(format!("GET {url} returned 304 without a validator"))),
    }
}

fn etag_path(cached: &Path) -> PathBuf {
    let mut path = cached.as_os_str().to_os_string();
    path.push(".etag");
    PathBuf::from(path)
}

fn cached_etag(cached: &Path) -> Option<String> {
    if !cached.is_file() {
        return None;
    }
    let value = fs::read_to_string(etag_path(cached)).ok()?;
    let value = value.trim();
    valid_etag(value).then(|| value.to_owned())
}

fn valid_etag(value: &str) -> bool {
    let tag = value.strip_prefix("W/").unwrap_or(value);
    tag.len() >= 2
        && tag.starts_with('"')
        && tag.ends_with('"')
        && tag[1..tag.len() - 1]
            .bytes()
            .all(|byte| byte == b'!' || (b'#'..=b'~').contains(&byte) || byte >= 0x80)
}

fn write_etag(cached: &Path, etag: Option<&str>) -> Result<()> {
    let path = etag_path(cached);
    if let Some(etag) = etag {
        atomic_write(&path, etag.as_bytes())
    } else if path.exists() {
        fs::remove_file(path).map_err(Into::into)
    } else {
        Ok(())
    }
}

fn atomic_write(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| error(format!("{} has no parent directory", path.display())))?;
    fs::create_dir_all(parent)?;
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    fs::write(&temporary, contents)?;
    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::rename(&temporary, path)?;
    Ok(())
}

fn supported_flags(upstream_path: &Path) -> Result<BTreeMap<String, String>> {
    let output = Command::new(upstream_path).arg("--show-flags").output()?;
    if !output.status.success() {
        return Err(error(format!(
            "{} --show-flags failed: {}",
            upstream_path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let stdout = String::from_utf8(output.stdout)?;
    let mut flags = BTreeMap::new();
    for line in stdout.lines().map(str::trim) {
        if let Some((name, value)) = line.split_once('=') {
            if valid_flag_name(name) && !value.is_empty() {
                flags.insert(name.to_owned(), value.to_owned());
            }
        }
    }
    if flags.is_empty() {
        return Err(error(format!(
            "{} --show-flags returned no parseable flags",
            upstream_path.display()
        )));
    }
    Ok(flags)
}

fn scraped_flags() -> Result<BTreeMap<String, String>> {
    let manifest = upstream()?;
    let contents = download(&manifest.roblox.fflags)?;
    let value: Value = serde_json::from_slice(&contents)?;
    let application = value
        .get("applicationSettings")
        .and_then(Value::as_object)
        .ok_or_else(|| error("Roblox FFlag response has no applicationSettings object"))?;
    let mut flags = BTreeMap::new();
    for (name, value) in application {
        let normalized = normalize_flag_name(name);
        if normalized != *name && valid_flag_name(&normalized) {
            flags.insert(normalized, value_string(value));
        }
    }
    Ok(flags)
}

fn apply_flag(
    output: &mut Flags,
    supported: &BTreeMap<String, String>,
    name: String,
    value: String,
    source: &str,
) {
    if !valid_flag_name(&name) || value.is_empty() {
        output
            .warnings
            .push(format!("ignored invalid FFlag {name:?} from {source}"));
    } else if supported.contains_key(&name) {
        output.values.insert(name, value);
    } else {
        output.warnings.push(format!(
            "{source} requested FFlag {name}, but bundled server {} does not expose it",
            upstream().map_or_else(|_| "<unknown>".to_owned(), |manifest| manifest.version)
        ));
    }
}

fn parse_pair(pair: &str) -> Result<(String, String)> {
    let (name, value) = pair
        .split_once('=')
        .ok_or_else(|| error(format!("invalid FFlag {pair:?}; expected KEY=VALUE")))?;
    let name = normalize_flag_name(name.trim());
    let value = value.trim();
    if !valid_flag_name(&name) || value.is_empty() {
        return Err(error(format!("invalid FFlag {pair:?}; expected KEY=VALUE")));
    }
    Ok((name, value.to_owned()))
}

fn normalize_flag_name(name: &str) -> String {
    for kind in FLAG_KINDS {
        if let Some(stripped) = name.strip_prefix(kind) {
            return stripped.to_owned();
        }
    }
    name.to_owned()
}

fn valid_flag_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn value_string(value: &Value) -> String {
    match value {
        Value::String(value) => value.trim().to_owned(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        other => other.to_string(),
    }
}

fn write_warning(message: &str) {
    use std::io::Write as _;

    let _result = writeln!(std::io::stderr().lock(), "luau-lsp: warning: {message}");
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::path::{Path, PathBuf};

    use serde::Deserialize;

    use super::{
        Fetch, UPSTREAM_JSON, cached_etag, etag_path, normalize_flag_name, parse_pair,
        revalidate_with, valid_etag, valid_flag_name,
    };
    use crate::Result;

    #[derive(Deserialize)]
    struct ReleaseManifest {
        repository: String,
        release_assets: BTreeMap<String, ReleaseAsset>,
    }

    #[derive(Deserialize)]
    struct ReleaseAsset {
        name: String,
        sha256: String,
        size: u64,
    }

    fn cache_directory(name: &str) -> Result<PathBuf> {
        let path = std::env::temp_dir().join(format!(
            "luau-lsp-roblox-assets-{name}-{}",
            std::process::id()
        ));
        if path.exists() {
            fs::remove_dir_all(&path)?;
        }
        fs::create_dir_all(&path)?;
        Ok(path)
    }

    #[test]
    fn strips_only_known_roblox_flag_kinds() {
        assert_eq!(normalize_flag_name("FFlagLuauSolverV2"), "LuauSolverV2");
        assert_eq!(normalize_flag_name("LuauSolverV2"), "LuauSolverV2");
    }

    #[test]
    fn validates_explicit_flag_pairs() -> Result<()> {
        assert_eq!(
            parse_pair("FFlagLuauSolverV2=true")?,
            ("LuauSolverV2".to_owned(), "true".to_owned())
        );
        assert!(valid_flag_name("LuauSolverV2"));
        assert!(!valid_flag_name("Luau-Solver"));
        Ok(())
    }

    #[test]
    fn release_assets_are_pinned() -> Result<()> {
        let manifest: ReleaseManifest = serde_json::from_str(UPSTREAM_JSON)?;
        let expected = BTreeSet::from([
            "linux-aarch64",
            "linux-x86_64",
            "macos-universal",
            "windows-x86_64",
        ]);
        let actual: BTreeSet<_> = manifest.release_assets.keys().map(String::as_str).collect();

        assert_eq!(manifest.repository, "JohnnyMorganz/luau-lsp");
        assert_eq!(actual, expected);
        for asset in manifest.release_assets.values() {
            assert!(
                Path::new(&asset.name)
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
            );
            assert_eq!(asset.sha256.len(), 64);
            assert!(asset.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()));
            assert!(asset.size > 0);
        }
        Ok(())
    }

    #[test]
    fn modified_resource_replaces_cache_and_etag() -> Result<()> {
        let directory = cache_directory("modified")?;
        let cached = directory.join("api-docs.json");
        fs::write(&cached, b"old")?;
        fs::write(etag_path(&cached), b"\"old\"")?;

        revalidate_with("https://example.invalid/resource", &cached, |url, etag| {
            assert_eq!(url, "https://example.invalid/resource");
            assert_eq!(etag, Some("\"old\""));
            Ok(Fetch::Modified {
                contents: b"new".to_vec(),
                etag: Some("\"new\"".to_owned()),
            })
        })?;

        assert_eq!(fs::read(&cached)?, b"new");
        assert_eq!(cached_etag(&cached).as_deref(), Some("\"new\""));
        fs::remove_dir_all(directory)?;
        Ok(())
    }

    #[test]
    fn not_modified_resource_preserves_cache() -> Result<()> {
        let directory = cache_directory("not-modified")?;
        let cached = directory.join("globalTypes.PluginSecurity.d.luau");
        fs::write(&cached, b"definitions")?;
        fs::write(etag_path(&cached), b"W/\"current\"")?;

        revalidate_with("https://example.invalid/resource", &cached, |_, etag| {
            assert_eq!(etag, Some("W/\"current\""));
            Ok(Fetch::NotModified)
        })?;

        assert_eq!(fs::read(&cached)?, b"definitions");
        assert_eq!(cached_etag(&cached).as_deref(), Some("W/\"current\""));
        fs::remove_dir_all(directory)?;
        Ok(())
    }

    #[test]
    fn rejects_malformed_etags() {
        assert!(valid_etag("\"strong\""));
        assert!(valid_etag("W/\"weak\""));
        assert!(!valid_etag("unquoted"));
        assert!(!valid_etag("\"line\nfeed\""));
    }
}

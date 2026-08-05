use std::env;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use clap::{Args as ClapArgs, Command, CommandFactory, Parser, ValueEnum};

use crate::{Result, error};

const UPSTREAM_ENV: &str = "LUAU_LSP_ROBLOX_UPSTREAM";
const CACHE_ENV: &str = "LUAU_LSP_ROBLOX_CACHE";

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum Platform {
    Roblox,
    Standard,
}

#[derive(ClapArgs, Debug)]
struct SyncOptions {
    /// Force Roblox fast-flag synchronization, which is enabled by default.
    #[arg(long = "sync-fflags", overrides_with = "no_sync")]
    sync: bool,

    /// Do not synchronize Roblox fast flags.
    #[arg(long = "no-sync-fflags", overrides_with = "sync")]
    no_sync: bool,
}

#[derive(ClapArgs, Debug)]
struct StudioOptions {
    /// Enable the Roblox Studio companion bridge.
    #[arg(long, overrides_with = "no_studio")]
    studio: bool,

    /// Disable the Roblox Studio companion bridge.
    #[arg(long = "no-studio", overrides_with = "studio")]
    no_studio: bool,
}

#[derive(ClapArgs, Debug)]
struct ControlOptions {
    /// Print wrapper help without intercepting upstream --help.
    #[arg(long = "wrapper-help")]
    help: bool,

    /// Print wrapper and bundled upstream versions.
    #[arg(long = "wrapper-version")]
    version: bool,
}

#[derive(ClapArgs, Debug)]
struct WrapperOptions {
    /// Override the Roblox security identity, which defaults to plugin security.
    #[arg(
        long = "roblox-security",
        value_name = "LEVEL",
        value_parser = [
            "None",
            "LocalUserSecurity",
            "PluginSecurity",
            "RobloxScriptSecurity"
        ]
    )]
    security: Option<String>,

    #[command(flatten)]
    sync: SyncOptions,

    #[command(flatten)]
    studio: StudioOptions,

    /// Load wrapper settings from this JSON file.
    #[arg(long = "wrapper-settings", value_name = "JSON_PATH")]
    settings: Option<PathBuf>,

    /// Store downloaded Roblox metadata in this directory.
    #[arg(long = "cache-dir", value_name = "PATH")]
    cache: Option<PathBuf>,

    /// Run this upstream luau-lsp executable.
    #[arg(long, value_name = "PATH")]
    upstream: Option<PathBuf>,

    #[command(flatten)]
    control: ControlOptions,
}

#[derive(Debug, Parser)]
#[command(
    name = "luau-lsp",
    about = "Editor-neutral Roblox runtime wrapper for luau-lsp",
    disable_help_flag = true,
    disable_version_flag = true,
    after_help = "All unrecognized arguments are forwarded to the bundled upstream executable."
)]
struct TransparentCli {
    #[command(flatten)]
    wrapper: WrapperOptions,

    /// Arguments forwarded to the bundled upstream executable.
    #[arg(value_name = "UPSTREAM_ARGUMENT")]
    forwarded: Vec<OsString>,
}

#[derive(Debug, Parser)]
#[command(
    name = "luau-lsp lsp",
    about = "Run luau-lsp with a managed Roblox environment",
    disable_help_flag = true,
    disable_version_flag = true,
    after_help = "Roblox definitions, documentation, PluginSecurity, FFlag synchronization, and Rojo sourcemaps are enabled by default. All unrecognized arguments are forwarded to the bundled upstream lsp command."
)]
struct LspCli {
    #[command(flatten)]
    wrapper: WrapperOptions,

    /// Select managed Roblox mode or transparent standard mode.
    #[arg(long, value_enum, default_value = "roblox")]
    platform: Platform,

    /// Arguments forwarded to the bundled upstream lsp command.
    #[arg(value_name = "UPSTREAM_ARGUMENT")]
    forwarded: Vec<OsString>,
}

#[derive(Debug, Parser)]
#[command(
    name = "luau-lsp analyze",
    about = "Analyze Luau with a managed Roblox environment",
    disable_help_flag = true,
    disable_version_flag = true,
    after_help = "Roblox definitions, PluginSecurity, FFlag synchronization, and Rojo sourcemaps are enabled by default. All unrecognized arguments are forwarded to the bundled upstream analyze command."
)]
struct AnalyzeCli {
    #[command(flatten)]
    wrapper: WrapperOptions,

    /// Select managed Roblox mode or transparent standard mode.
    #[arg(long, value_enum, default_value = "roblox")]
    platform: Platform,

    /// Arguments forwarded to the bundled upstream analyze command.
    #[arg(value_name = "UPSTREAM_ARGUMENT")]
    forwarded: Vec<OsString>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    Lsp,
    Analyze,
    Other,
}

#[derive(Debug)]
pub struct Args {
    pub(crate) forwarded: Vec<OsString>,
    pub(crate) platform: Platform,
    pub(crate) security: Option<String>,
    pub(crate) sync: Option<bool>,
    pub(crate) studio: Option<bool>,
    pub(crate) settings: Option<PathBuf>,
    pub(crate) cache: Option<PathBuf>,
    pub(crate) upstream: Option<PathBuf>,
    pub(crate) mode: Mode,
    pub(crate) help: bool,
    pub(crate) version: bool,
}

impl Args {
    pub(crate) fn parse() -> Result<Self> {
        Self::parse_from(env::args_os().skip(1))
    }

    fn parse_from(arguments: impl IntoIterator<Item = OsString>) -> Result<Self> {
        let input: Vec<OsString> = arguments.into_iter().collect();
        let mode = match input.first().and_then(|argument| argument.to_str()) {
            Some("lsp") => Mode::Lsp,
            Some("analyze") => Mode::Analyze,
            _ => Mode::Other,
        };
        match mode {
            Mode::Lsp => {
                let command = LspCli::command();
                let (wrapper, forwarded) = route_arguments(input.into_iter().skip(1), &command);
                let parsed = LspCli::try_parse_from(
                    std::iter::once(OsString::from("luau-lsp lsp"))
                        .chain(wrapper)
                        .chain(std::iter::once(OsString::from("--")))
                        .chain(forwarded),
                )?;
                Ok(Self::from_options(
                    parsed.wrapper,
                    std::iter::once(OsString::from("lsp"))
                        .chain(parsed.forwarded)
                        .collect(),
                    parsed.platform,
                    mode,
                ))
            }
            Mode::Analyze => {
                let command = AnalyzeCli::command();
                let (wrapper, forwarded) = route_arguments(input.into_iter().skip(1), &command);
                let parsed = AnalyzeCli::try_parse_from(
                    std::iter::once(OsString::from("luau-lsp analyze"))
                        .chain(wrapper)
                        .chain(std::iter::once(OsString::from("--")))
                        .chain(forwarded),
                )?;
                let mut forwarded = std::iter::once(OsString::from("analyze"))
                    .chain(parsed.forwarded)
                    .collect::<Vec<_>>();
                if parsed.platform == Platform::Standard {
                    forwarded.insert(1, OsString::from("--platform=standard"));
                }
                Ok(Self::from_options(
                    parsed.wrapper,
                    forwarded,
                    parsed.platform,
                    mode,
                ))
            }
            Mode::Other => {
                let command = TransparentCli::command();
                let (wrapper, forwarded) = route_arguments(input, &command);
                let parsed = TransparentCli::try_parse_from(
                    std::iter::once(OsString::from("luau-lsp"))
                        .chain(wrapper)
                        .chain(std::iter::once(OsString::from("--")))
                        .chain(forwarded),
                )?;
                Ok(Self::from_options(
                    parsed.wrapper,
                    parsed.forwarded,
                    Platform::Roblox,
                    mode,
                ))
            }
        }
    }

    fn from_options(
        options: WrapperOptions,
        forwarded: Vec<OsString>,
        platform: Platform,
        mode: Mode,
    ) -> Self {
        let sync = option_pair(options.sync.sync, options.sync.no_sync);
        let studio = option_pair(options.studio.studio, options.studio.no_studio);
        Self {
            forwarded,
            platform,
            security: options.security,
            sync,
            studio,
            settings: options.settings,
            cache: options.cache,
            upstream: options.upstream,
            mode,
            help: options.control.help,
            version: options.control.version,
        }
    }

    pub(crate) fn help_text(&self) -> String {
        let mut command = match self.mode {
            Mode::Lsp => LspCli::command(),
            Mode::Analyze => AnalyzeCli::command(),
            Mode::Other => TransparentCli::command(),
        };
        command.render_long_help().to_string()
    }

    pub(crate) fn managed(&self) -> bool {
        self.mode != Mode::Other && self.platform == Platform::Roblox
    }

    pub(crate) fn upstream_path(&self) -> Result<PathBuf> {
        let path = if let Some(path) = &self.upstream {
            path.clone()
        } else if let Some(path) = env::var_os(UPSTREAM_ENV) {
            PathBuf::from(path)
        } else {
            let executable = env::current_exe()?;
            let directory = executable
                .parent()
                .ok_or_else(|| error("the wrapper executable has no parent directory"))?;
            directory.join(upstream_name())
        };

        if path.is_file() {
            Ok(path)
        } else {
            Err(error(format!(
                "bundled upstream executable not found at {}",
                path.display()
            )))
        }
    }

    pub(crate) fn cache_path(&self) -> Result<PathBuf> {
        if let Some(path) = &self.cache {
            return Ok(path.clone());
        }
        if let Some(path) = env::var_os(CACHE_ENV) {
            return Ok(PathBuf::from(path));
        }

        platform_cache().map(|path| path.join("luau-lsp-roblox"))
    }
}

fn route_arguments(
    arguments: impl IntoIterator<Item = OsString>,
    command: &Command,
) -> (Vec<OsString>, Vec<OsString>) {
    let mut arguments = arguments.into_iter();
    let mut wrapper = Vec::new();
    let mut forwarded = Vec::new();

    while let Some(argument) = arguments.next() {
        if argument == OsStr::new("--") {
            forwarded.push(argument);
            forwarded.extend(arguments);
            break;
        }

        let Some(text) = argument.to_str() else {
            forwarded.push(argument);
            continue;
        };
        let Some(option) = text.strip_prefix("--") else {
            forwarded.push(argument);
            continue;
        };
        let (name, inline) = option
            .split_once('=')
            .map_or((option, false), |(name, _)| (name, true));
        let definition = command.get_arguments().find(|candidate| {
            candidate
                .get_long_and_visible_aliases()
                .is_some_and(|names| names.contains(&name))
        });
        let Some(definition) = definition else {
            forwarded.push(argument);
            continue;
        };

        wrapper.push(argument);
        if !inline
            && definition.get_action().takes_values()
            && let Some(value) = arguments.next()
        {
            wrapper.push(value);
        }
    }

    (wrapper, forwarded)
}

const fn option_pair(enabled: bool, disabled: bool) -> Option<bool> {
    if enabled {
        Some(true)
    } else if disabled {
        Some(false)
    } else {
        None
    }
}

#[cfg(windows)]
const fn upstream_name() -> &'static str {
    "luau-lsp-server.exe"
}

#[cfg(not(windows))]
const fn upstream_name() -> &'static str {
    "luau-lsp-server"
}

#[cfg(windows)]
fn platform_cache() -> Result<PathBuf> {
    env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .ok_or_else(|| error("LOCALAPPDATA is not set; use --cache-dir"))
}

#[cfg(not(windows))]
fn platform_cache() -> Result<PathBuf> {
    if let Some(path) = env::var_os("XDG_CACHE_HOME") {
        return Ok(PathBuf::from(path));
    }
    env::var_os("HOME")
        .map(PathBuf::from)
        .map(|path| path.join(".cache"))
        .ok_or_else(|| error("HOME is not set; use --cache-dir"))
}

pub fn sibling_resource(path: &Path) -> Result<PathBuf> {
    path.parent()
        .map(|directory| directory.join("resources"))
        .ok_or_else(|| error("the upstream executable has no parent directory"))
}

#[cfg(test)]
mod tests {
    use super::{Args, Mode, Platform};
    use crate::Result;
    use std::ffi::OsString;

    fn parse(arguments: &[&str]) -> Result<Args> {
        let owned = arguments.iter().map(OsString::from);
        Args::parse_from(owned)
    }

    #[test]
    fn wrapper_options_are_removed_from_lsp_arguments() -> Result<()> {
        let arguments = parse(&[
            "lsp",
            "--platform",
            "roblox",
            "--roblox-security=PluginSecurity",
            "--sync-fflags",
            "--docs=user.json",
        ])?;

        assert!(arguments.managed());
        assert_eq!(arguments.platform, Platform::Roblox);
        assert_eq!(arguments.security.as_deref(), Some("PluginSecurity"));
        assert_eq!(arguments.sync, Some(true));
        assert_eq!(
            arguments.forwarded,
            [OsString::from("lsp"), OsString::from("--docs=user.json")]
        );
        Ok(())
    }

    #[test]
    fn standard_analysis_remains_transparent() -> Result<()> {
        let arguments = parse(&["analyze", "--platform", "roblox", "main.luau"])?;

        assert!(arguments.managed());
        assert_eq!(arguments.mode, Mode::Analyze);
        assert_eq!(
            arguments.forwarded,
            [OsString::from("analyze"), OsString::from("main.luau")]
        );
        let arguments = parse(&["analyze", "--platform", "standard", "main.luau"])?;
        assert!(!arguments.managed());
        assert_eq!(
            arguments.forwarded,
            [
                OsString::from("analyze"),
                OsString::from("--platform=standard"),
                OsString::from("main.luau")
            ]
        );
        Ok(())
    }

    #[test]
    fn source_names_do_not_change_analysis_mode() -> Result<()> {
        let arguments = parse(&["analyze", "lsp"])?;

        assert!(arguments.managed());
        assert_eq!(arguments.mode, Mode::Analyze);
        assert_eq!(arguments.forwarded.len(), 2);
        Ok(())
    }

    #[test]
    fn upstream_help_is_forwarded() -> Result<()> {
        let arguments = parse(&["lsp", "--help"])?;

        assert!(!arguments.help);
        assert_eq!(
            arguments.forwarded,
            [OsString::from("lsp"), OsString::from("--help")]
        );
        Ok(())
    }

    #[test]
    fn wrapper_options_remain_available_after_upstream_arguments() -> Result<()> {
        let arguments = parse(&[
            "lsp",
            "--docs=user.json",
            "workspace",
            "--platform",
            "standard",
            "--studio",
        ])?;

        assert_eq!(arguments.platform, Platform::Standard);
        assert_eq!(arguments.studio, Some(true));
        assert_eq!(
            arguments.forwarded,
            [
                OsString::from("lsp"),
                OsString::from("--docs=user.json"),
                OsString::from("workspace")
            ]
        );
        Ok(())
    }

    #[test]
    fn invalid_wrapper_values_are_rejected() {
        let arguments = ["lsp", "--platform", "invalid"]
            .into_iter()
            .map(OsString::from);

        assert!(Args::parse_from(arguments).is_err());
    }
}

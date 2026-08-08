# luau-lsp-roblox

`luau-lsp-roblox` publishes an editor-neutral `luau-lsp` command. The public
executable is a Rust session wrapper; a compatible upstream `luau-lsp` binary is
stored privately beside it as `luau-lsp-server`.

For Roblox language-server sessions the wrapper:

- supplies Roblox definitions and API documentation;
- discovers the bundled server's actual FFlag registry and overlays only matching
  values from Roblox's published Studio settings;
- normalizes nested and dotted editor configuration;
- owns Rojo sourcemap generation and notifies the server when the generated file
  changes;
- hosts the existing Studio companion protocol on loopback; and
- forwards every LSP message it does not need to adapt.

The `analyze` command uses the same managed Roblox definitions and FFlags. It
also finds the nearest Rojo project for the requested files and generates a
current sourcemap before analysis. Other upstream modes are launched unchanged.

## Install

### mise

Release archives use conventional operating-system and architecture names so the
mise GitHub backend can select them directly:

```text
mise use github:0hirume/luau-lsp-roblox
```

### Manual

Download the archive for your operating system and architecture from the
[latest release](https://github.com/0hirume/luau-lsp-roblox/releases/latest),
then extract its contents into a directory on `PATH`.

Managed Roblox mode, `PluginSecurity`, Roblox definitions and documentation,
Solver V2, dynamic FFlag synchronization, and automatic Rojo sourcemaps are
enabled by default. Use `--platform standard` for an unmanaged, transparent
upstream command. The Studio companion bridge is optional and can be enabled
with `--studio`.

## Analyze

Run analysis from the project root without manually locating cached definitions
or a sourcemap:

```text
luau-lsp analyze places/earth/src/shared/rig.luau
```

The wrapper supplies `LuauSolverV2`, the selected Roblox security definitions,
and a one-shot Rojo sourcemap automatically. Explicit upstream `--flag`,
`--definitions`, `--settings`, and `--sourcemap` options still take precedence.
Files from multiple Rojo roots require an explicit `--sourcemap`.

## Wrapper options

The wrapper removes these options before launching the private server:

```text
--platform <roblox|standard>
--roblox-security <None|LocalUserSecurity|PluginSecurity|RobloxScriptSecurity>
--sync-fflags | --no-sync-fflags
--studio | --no-studio
--wrapper-settings <JSON path>
--cache-dir <path>
--upstream <path>
--wrapper-version
--wrapper-help
```

Every other argument is forwarded. `--upstream` is intended for development; a
release normally resolves `luau-lsp-server` beside the wrapper. The equivalent
environment overrides are `LUAU_LSP_ROBLOX_UPSTREAM` and
`LUAU_LSP_ROBLOX_CACHE`.

`--wrapper-settings` accepts dotted VS Code keys, a nested `luau-lsp` object, or
the section value itself. Editors can provide the same shapes in
`initializationOptions.settings`. Partial, unsupported, and unclassified
settings produce compatibility notices; managed platform and loaded definitions
remain under wrapper control. Restart the LSP session after changing a setting
that controls a startup-owned resource.

See the [complete configuration reference](CONFIGURATION.md) for every
upstream setting, its default, scope, and wrapper compatibility. Neovim users
can start with the [Neovim section](CONFIGURATION.md#neovim).

## Sourcemaps and Studio

The Rojo executable remains an external dependency. For an LSP session the
wrapper finds `default.project.json`, runs:

```text
rojo sourcemap default.project.json --output sourcemap.json --include-non-scripts --watch
```

and independently observes `sourcemap.json`. `analyze` discovers the nearest
project from its input paths and runs the same command once without `--watch`.
Custom paths and generator commands use the settings classified in
[`upstream/compatibility.json`](upstream/compatibility.json).

The Studio bridge binds only to `127.0.0.1`. It preserves `/full`, `/clear`, and
`/get-file-paths` from the upstream VS Code adapter and stops with the LSP session.

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

Other upstream modes are launched unchanged.

## Install

Release archives use conventional operating-system and architecture names so the
mise GitHub backend can select them directly:

```text
mise use github:0hirume/luau-lsp-roblox
```

Managed Roblox mode, `PluginSecurity`, Roblox definitions and documentation,
dynamic FFlag synchronization, and automatic Rojo sourcemaps are enabled by
default. Use `--platform standard` for an unmanaged, transparent upstream
session. The Studio companion bridge is optional and can be enabled with
`--studio`.

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
`initializationOptions.settings`. Wrapper-owned startup settings discovered only
after initialization are reported as requiring a session restart instead of being
silently ignored.

## Sourcemaps and Studio

The Rojo executable remains an external dependency. By default the wrapper finds
`default.project.json`, runs:

```text
rojo sourcemap default.project.json --output sourcemap.json --include-non-scripts --watch
```

and independently observes `sourcemap.json`. Custom paths and generator commands
use the settings classified in [`upstream/compatibility.json`](upstream/compatibility.json).

The Studio bridge binds only to `127.0.0.1`. It preserves `/full`, `/clear`, and
`/get-file-paths` from the upstream VS Code adapter and stops with the LSP session.

## Development

The project treats warnings as errors and enables Clippy's `all`, `pedantic`, and
`nursery` groups together with explicit bans on unsafe code, panics, unwraps,
expects, TODOs, and unimplemented paths. Lint suppressions are not used.

```text
cargo fmt --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --all-features --locked
```

`cargo release <level> --execute` bumps and commits the version, creates a
`v<version>` tag, and pushes the commit and tag to `origin`. The pushed tag starts
the GitHub Release workflow. This package cannot be published to a Cargo registry.

Pinned upstream metadata and release-asset hashes live in
[`upstream/manifest.json`](upstream/manifest.json). Run `nu scripts/upstream.nu`
to update to the latest stable tag, or pass an exact version. The updater preserves
classifications only for settings whose schemas did not change; added or changed
settings must be explicitly classified before `nu scripts/upstream.nu --check`
and the Rust test suite pass.

# luau-lsp-roblox

## Motivation

Using luau-lsp well for Roblox currently depends on machinery owned by its VS Code extension. Other editors must reproduce Roblox definitions, documentation, FFlags, sourcemaps, live reload behavior, and Studio integration themselves. That creates editor-specific setup, stale generated state, and unnecessary language-server restarts.

This project should take that environment management out of VS Code and make it available to every LSP-capable editor.

## Goal

Build an editor-neutral Rust distribution and runtime wrapper for luau-lsp's Roblox platform.

A user should be able to install it from GitHub Releases with mise, point any editor at the installed `luau-lsp` command, and get a complete Roblox-aware language-server environment without editor extensions or project scaffolding.

The intended interface includes:

```text
luau-lsp lsp --platform roblox --roblox-security PluginSecurity --sync-fflags
```

## Required behavior

The tool should:

- Bundle and launch a compatible upstream luau-lsp executable.
- Behave as a drop-in `luau-lsp` command and pass through upstream functionality that it does not own.
- Obtain and supply the Roblox type definitions and documentation expected by luau-lsp.
- Configure the requested Roblox security level.
- Synchronize FFlags dynamically against the capabilities of the bundled server.
- Own sourcemap generation and keep it current for the language-server session.
- Detect sourcemap changes independently of the editor and notify luau-lsp without restarting it.
- Provide the existing Studio companion integration outside VS Code so live DataModel information works with any editor.
- Honor the configuration advertised by the pinned upstream release as consistently as possible across editors.
- Preserve ordinary luau-lsp configuration and protocol behavior.
- Leave room to expose useful upstream inspection features that currently only have a VS Code frontend.

## Configuration compatibility

The configuration advertised by the pinned upstream VS Code package is part of the compatibility target. Some of those settings are implemented by the language server, while others are interpreted by the extension, translated into startup arguments or initialization options, or completed through VS Code-specific behavior. A generic client can therefore provide valid-looking configuration that is accepted but has no effect.

The wrapper should make that contract honest across editors. For every advertised setting, determine whether it is:

- implemented directly by the server and should be forwarded;
- implemented by the VS Code adapter and should be reproduced by the wrapper;
- only partially portable and should be supported when the client's standard LSP capabilities permit it;
- inherently editor-specific and cannot be supported by the wrapper.

Recognized settings must not fail silently. Partial or unsupported behavior should be reported clearly rather than pretending that the setting works.

Accept and normalize the common configuration shapes used by editors, including nested LSP settings and the dotted keys advertised by the VS Code schema. Preserve unrelated server settings while applying wrapper-owned behavior at the correct stage of server startup or protocol handling.

Keep the compatibility classification tied to the bundled upstream version. When the upstream schema changes, new or changed settings must be reviewed and classified instead of being silently omitted.

Features such as autocomplete-end illustrate the limit: the wrapper should reproduce their semantics through standard LSP when possible, but it must not claim complete support when the behavior fundamentally depends on a private editor command.

## FFlag rule

FFlag handling must remain fully dynamic.

- Use the bundled server's `--show-flags` output as the authority for the flags it supports and their server-provided values.
- Obtain current Roblox values from the scraped source.
- Apply scraped values only to matching flags exposed by the installed server.
- Do not maintain a duplicated flag allowlist.
- If a supported flag is absent from the scraped source, do not invent a scraped value for it; preserve the server-provided behavior.
- Treat schema settings that represent FFlag behavior as a separate compatibility layer rather than part of scraped synchronization.
- In particular, `fflags.enableNewSolver` should enable `LuauSolverV2` when that flag is exposed by the bundled server, and should report that it is unsupported when the flag is absent.
- Allow explicit user overrides to take precedence when provided.

The supported flag registry and Roblox-value overlay must remain dynamic as Roblox and luau-lsp add, remove, or rename flags. Schema aliases may name particular semantics because matching the advertised schema requires them, but they must be checked against the installed server rather than assumed to exist.

## Architecture boundary

The public executable should be the Rust wrapper named `luau-lsp`. The upstream executable should be bundled privately under a different name and launched by the wrapper.

```text
editor <-> Rust luau-lsp wrapper <-> bundled upstream luau-lsp
```

The wrapper may intercept or inject LSP messages only where required to provide editor-neutral Roblox behavior. Everything else should pass through transparently.

Keep protocol output clean, preserve upstream settings, manage owned child processes with the session, and avoid making any editor responsible for wrapper internals.

Do not fork or reimplement the language server. Do not link the C++ server into Rust. Keep upstream replaceable by changing the bundled version.

## Sourcemaps

The wrapper should own the sourcemap lifecycle rather than merely generate a file once.

It should discover or accept the relevant Rojo project, run the configured sourcemap generator, keep it alive when watching is supported, observe the generated sourcemap itself, and tell luau-lsp when that file changes.

The result must not depend on an editor correctly reporting external filesystem changes. A normal sourcemap update must not require restarting the editor or language server.

Rojo can remain an external dependency initially. The wrapper should allow its location or the generator command to be configured.

## Studio integration

Move the useful, editor-neutral Studio companion bridge behavior out of the VS Code adapter.

The existing Studio plugin should be able to connect to the wrapper, obtain workspace Luau file paths, and send live DataModel updates. The wrapper should forward those updates through the protocol already supported by luau-lsp.

Preserve compatibility with the existing plugin where practical. Keep the bridge local to the machine and tie its lifetime to the language-server session.

## Inspection features

Luau-lsp already exposes compiler and workspace inspection capabilities that VS Code presents through custom UI, including bytecode, compiler remarks, native code generation, internal source, and require graphs.

These are useful follow-up capabilities, but they must not block the core Roblox environment work. Expose them later through editor-neutral output or commands rather than recreating VS Code webviews.

## Distribution

Publish prebuilt GitHub Release archives for the platforms supported by the selected upstream luau-lsp release.

Each archive should contain:

- The public Rust `luau-lsp` wrapper.
- The pinned private upstream server executable.
- Version metadata for both components.
- Required license notices.

Release assets should use conventional OS and architecture names so mise's GitHub backend can select them automatically.

Installation should work without a custom mise plugin:

```text
mise use github:<owner>/luau-lsp-roblox
```

Upgrading the bundled upstream server should produce a new release rather than silently changing an existing version.

## Non-goals

- Do not add Helix-specific behavior.
- Do not recreate VS Code settings UI, webviews, syntax support, or command-palette behavior.
- Do not absorb Helve, roforgecloud, or unrelated scaffolding responsibilities.
- Do not reimplement settings already handled correctly by the server; forward them.
- Do not hardcode a snapshot of supported FFlags.
- Do not prescribe internal modules, crates, retry counts, cache layouts, or other implementation details before they are needed.

## Completion criteria

The task is complete when:

- The tool installs from a GitHub Release through mise.
- Any LSP-capable editor can launch its `luau-lsp` executable.
- Roblox definitions, documentation, security, and FFlags are supplied without editor-specific setup.
- FFlag synchronization adapts dynamically to the bundled server and scraped Roblox data.
- `fflags.enableNewSolver` and other portable adapter-owned settings work outside VS Code when supported by the bundled server.
- Every setting in the pinned upstream schema is classified as forwarded, adapted, partial, or unsupported.
- Common editor configuration shapes are normalized consistently, and recognized settings never fail silently.
- Sourcemaps regenerate and reload without restarting luau-lsp.
- The existing Studio companion functionality works without VS Code.
- Ordinary luau-lsp behavior and configuration continue to pass through correctly.
- The wrapper and every process it owns shut down cleanly with the LSP session.

Implementation details should be chosen by inspecting the current upstream server, VS Code adapter, Studio plugin, and the existing scaffold before writing each subsystem. Preserve behavior that is required for compatibility, but prefer the smallest editor-neutral design that achieves these outcomes.

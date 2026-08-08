# Configuration

This is the complete configuration reference for the bundled upstream
`luau-lsp` 1.69.0 server at commit
`2f42ede46764cfcc6ad569a395c38bbfd3878342`. The setting names, types, defaults,
scopes, and descriptions come from [`upstream/schema.json`](upstream/schema.json).
The compatibility column is maintained in
[`upstream/compatibility.json`](upstream/compatibility.json).

The setting tables use the upstream schema default. Managed Roblox defaults are
listed separately below.

For a Neovim setup, see the [Neovim section](#neovim).

## Configuration sources

Wrapper settings are used for managed Roblox `lsp` and `analyze` sessions. A
standard-mode session is transparent: upstream arguments and configuration are
passed through without the managed Roblox layer.

The wrapper accepts settings from these sources:

- `--settings <path>` or `--settings=<path>`: an upstream settings file passed
  as a forwarded argument. `--settings:path` is also accepted.
- `--wrapper-settings <path>`: a settings file read by the wrapper.
- LSP `initializationOptions.settings`, or embedded `luau-lsp` and `luau`
  settings in `initializationOptions`.
- LSP `workspace/didChangeConfiguration` notifications after startup.

Settings files may be a direct object or contain a top-level `settings` object.
These equivalent forms are accepted:

```json
{
  "luau-lsp.sourcemap.enabled": false,
  "luau-lsp.fflags.override": {
    "LuauSolverV2": "true"
  }
}
```

```json
{
  "luau-lsp": {
    "sourcemap": {
      "enabled": false
    },
    "fflags": {
      "override": {
        "LuauSolverV2": "true"
      }
    }
  }
}
```

```json
{
  "settings": {
    "sourcemap": {
      "enabled": false
    }
  }
}
```

The `luau` namespace is accepted for upstream `luau.*` settings such as
`luau.trace.server`. Keys without a namespace are interpreted as
`luau-lsp.*` settings.

## Precedence and lifecycle

For a managed session, the effective settings are merged in this order:

1. Managed wrapper defaults.
2. The forwarded upstream `--settings` file.
3. The `--wrapper-settings` file.
4. LSP initialization settings.
5. Wrapper CLI overrides: `--platform`, `--roblox-security`,
   `--sync-fflags` or `--no-sync-fflags`, and `--studio` or `--no-studio`.

Later values replace earlier values for the same setting. Object-valued
settings such as `fflags.override` are replaced as a setting; they are not
deep-merged. Explicit upstream arguments such as `--definitions` and
`--sourcemap` continue to take precedence over automatic managed behavior.

Managed mode forces `luau-lsp.platform.type` to `roblox`. The wrapper also
keeps the definitions it loaded for the session in
`luau-lsp.types.definitionFiles`, so a later configuration response cannot
replace those definitions.

Configuration changes are normalized into the nested shape expected by the
upstream server. Partial, unsupported, and unclassified settings produce a log
notice that the LSP session may need to be restarted. The managed platform and
loaded definitions remain under wrapper control, and the wrapper does not
silently rebuild downloaded definitions, FFlags, or other startup resources in
place.

## Managed defaults

The following values are supplied by the wrapper baseline in managed Roblox
sessions. The upstream schema defaults apply to the remaining settings.

`luau-lsp.fflags.enableNewSolver` is the only explicit managed baseline value
that differs from its upstream schema default: managed Roblox mode changes it
from `false` to `true`. `analyze` writes `fflags.enableByDefault=true` and
`fflags.sync=false` to its temporary settings file only after resolving FFlags;
those are internal post-resolution values, not additional user-facing defaults.

| Setting                                | Managed value          | Effect                                                 |
| -------------------------------------- | ---------------------- | ------------------------------------------------------ |
| `luau-lsp.platform.type`               | `roblox`               | Selects managed Roblox behavior.                       |
| `luau-lsp.sourcemap.enabled`           | `true`                 | Enables sourcemap monitoring and generation.           |
| `luau-lsp.sourcemap.autogenerate`      | `true`                 | Runs the configured sourcemap generator.               |
| `luau-lsp.sourcemap.rojoProjectFile`   | `default.project.json` | Finds the Rojo project to generate.                    |
| `luau-lsp.sourcemap.includeNonScripts` | `true`                 | Includes non-script instances in generated maps.       |
| `luau-lsp.sourcemap.sourcemapFile`     | `sourcemap.json`       | Monitors and supplies this sourcemap.                  |
| `luau-lsp.sourcemap.useVSCodeWatcher`  | `false`                | Uses the wrapper's editor-neutral polling behavior.    |
| `luau-lsp.fflags.enableByDefault`      | `false`                | Leaves unspecified boolean FFlags disabled at startup. |
| `luau-lsp.fflags.enableNewSolver`      | `true`                 | Enables `LuauSolverV2` when the server advertises it.  |
| `luau-lsp.fflags.sync`                 | `true`                 | Synchronizes compatible published Roblox Luau FFlags.  |
| `luau-lsp.types.roblox`                | `true`                 | Loads managed Roblox definitions.                      |
| `luau-lsp.types.robloxSecurityLevel`   | `PluginSecurity`       | Uses the PluginSecurity definition set.                |
| `luau-lsp.studioPlugin.enabled`        | `false`                | Leaves the loopback Studio companion disabled.         |

For `analyze`, the wrapper applies the requested FFlags first and writes a
temporary upstream settings file with `fflags.enableByDefault=true` and
`fflags.sync=false`, because those values have already been resolved by the
wrapper.

## Compatibility classes

The registry currently covers all 90 schema settings: 56 are `forwarded`, 26
are `adapted`, 4 are `partial`, and 4 are `unsupported`.

- **forwarded** — the upstream server owns the behavior and receives the
  setting unchanged.
- **adapted** — the wrapper consumes or translates the setting and may also
  forward a normalized value.
- **partial** — only the portable part can be reproduced by an editor-neutral
  wrapper.
- **unsupported** — the setting belongs to the editor or cannot be reproduced
  by this distribution.

The registry contains the exact rationale for each classification. The tables
below include every schema key and its classification.

## Server and platform

| Setting                                  | Type and allowed values              | Upstream default   | Scope    | Compatibility | Description                                                                                                                           |
| ---------------------------------------- | ------------------------------------ | ------------------ | -------- | ------------- | ------------------------------------------------------------------------------------------------------------------------------------- |
| `luau.trace.server`                      | string: `off`, `messages`, `verbose` | `off`              | window   | unsupported   | Traces communication between the editor and language server. Use Neovim or editor tracing instead.                                    |
| `luau-lsp.server.path`                   | string                               | `""`               | —        | unsupported   | Path to the Luau LSP server binary. The distribution owns its bundled server; use `--upstream` for wrapper development.               |
| `luau-lsp.server.communicationChannel`   | string: `stdio`, `pipe`              | `stdio`            | —        | partial       | Selects the server communication channel. Managed Roblox sessions require stdio; standard sessions can pass pipe arguments through.   |
| `luau-lsp.server.delayStartup`           | boolean                              | `false`            | —        | adapted       | Keeps the server spinning at startup for debugger attachment.                                                                         |
| `luau-lsp.server.crashReporting.enabled` | boolean                              | `false`            | —        | adapted       | Uploads crash reports to Sentry when the bundled server supports the required arguments.                                              |
| `luau-lsp.server.baseLuaurc`             | string                               | —                  | window   | adapted       | Path to a `.luaurc` file used as the baseline Luau configuration.                                                                     |
| `luau-lsp.ignoreGlobs`                   | array of strings                     | `["**/_Index/**"]` | resource | forwarded     | Suppresses diagnostics for matching files unless the file is open.                                                                    |
| `luau-lsp.platform.type`                 | string: `standard`, `roblox`         | `roblox`           | window   | adapted       | Selects platform-specific support. Managed mode forces this to `roblox`; use `--platform standard` for transparent upstream behavior. |

## Sourcemaps

| Setting                                | Type and allowed values | Upstream default       | Scope    | Compatibility | Description                                                                                                         |
| -------------------------------------- | ----------------------- | ---------------------- | -------- | ------------- | ------------------------------------------------------------------------------------------------------------------- |
| `luau-lsp.sourcemap.enabled`           | boolean                 | `true`                 | resource | adapted       | Enables Rojo sourcemap parsing, wrapper monitoring, and automatic generation.                                       |
| `luau-lsp.sourcemap.autogenerate`      | boolean                 | `true`                 | resource | adapted       | Runs `rojo sourcemap` or the configured generator when the project changes.                                         |
| `luau-lsp.sourcemap.rojoPath`          | string                  | —                      | resource | adapted       | Path to the Rojo executable. The wrapper uses `rojo` when this is unset.                                            |
| `luau-lsp.sourcemap.rojoProjectFile`   | string                  | `default.project.json` | resource | adapted       | Rojo project file used for sourcemap generation.                                                                    |
| `luau-lsp.sourcemap.includeNonScripts` | boolean                 | `true`                 | resource | adapted       | Adds Rojo's `--include-non-scripts` option when generating a map.                                                   |
| `luau-lsp.sourcemap.sourcemapFile`     | string                  | `sourcemap.json`       | resource | adapted       | Generated sourcemap file monitored by the wrapper and read by the server.                                           |
| `luau-lsp.sourcemap.generatorCommand`  | string                  | —                      | resource | adapted       | Custom generator command. It is split into arguments and launched without a shell.                                  |
| `luau-lsp.sourcemap.useVSCodeWatcher`  | boolean                 | `false`                | resource | adapted       | Uses the wrapper's workspace poller to rerun the generator instead of delegating watching to the generator process. |

The default generator command is equivalent to:

```text
rojo sourcemap <project-file> --output <sourcemap-file> --include-non-scripts --watch
```

When `generatorCommand` is set, the command is used as written and the wrapper
does not append Rojo-specific arguments. The `rojoPath`, project, output, and
watcher settings still control automatic discovery and monitoring.

## Formatting, FFlags, and diagnostics

| Setting                                     | Type and allowed values | Upstream default | Scope    | Compatibility | Description                                                                                                 |
| ------------------------------------------- | ----------------------- | ---------------- | -------- | ------------- | ----------------------------------------------------------------------------------------------------------- |
| `luau-lsp.format.convertQuotes`             | boolean                 | `false`          | resource | partial       | Converts quote strings to backticks when typing `{`; the private editor cursor command is not portable.     |
| `luau-lsp.fflags.enableByDefault`           | boolean                 | `false`          | window   | adapted       | Enables all boolean Luau FFlags by default before overrides and synchronization.                            |
| `luau-lsp.fflags.enableNewSolver`           | boolean                 | `false`          | window   | adapted       | Enables the flags required by Luau's new type solver. Managed mode defaults this to `true`.                 |
| `luau-lsp.fflags.sync`                      | boolean                 | `true`           | window   | adapted       | Synchronizes published Roblox FFlags whose normalized names are supported by the bundled server.            |
| `luau-lsp.fflags.override`                  | object of string values | `{}`             | window   | adapted       | Overrides FFlags after synchronization. Boolean and number JSON values are converted to strings.            |
| `luau-lsp.diagnostics.includeDependents`    | boolean                 | `true`           | resource | forwarded     | Recomputes dependent diagnostics when a file changes. Ignored when workspace diagnostics are enabled.       |
| `luau-lsp.diagnostics.workspace`            | boolean                 | `false`          | resource | forwarded     | Computes diagnostics for the whole workspace.                                                               |
| `luau-lsp.diagnostics.strictDatamodelTypes` | boolean                 | `false`          | resource | forwarded     | Uses strict DataModel types for diagnostics instead of treating `game`, `script`, and `workspace` as `any`. |
| `luau-lsp.diagnostics.pullOnChange`         | boolean                 | `true`           | resource | unsupported   | Requests document diagnostics whenever the text changes. Scheduling belongs to the LSP client.              |
| `luau-lsp.diagnostics.pullOnSave`           | boolean                 | `true`           | resource | unsupported   | Requests document diagnostics whenever the file is saved. Scheduling belongs to the LSP client.             |

FFlag names may be written with Roblox prefixes such as `FFlag`; the wrapper
normalizes them before checking the bundled server's `--show-flags` registry.
Unsupported or invalid names produce a warning and are ignored.

## Types and Roblox definitions

| Setting                              | Type and allowed values                                                       | Upstream default | Scope  | Compatibility | Description                                                                                                                         |
| ------------------------------------ | ----------------------------------------------------------------------------- | ---------------- | ------ | ------------- | ----------------------------------------------------------------------------------------------------------------------------------- |
| `luau-lsp.types.definitionFiles`     | object of string paths, or wrapper-accepted path array                        | `{}`             | window | adapted       | Maps package names to definition files loaded by the type checker. Relative paths are resolved against the workspace when possible. |
| `luau-lsp.types.documentationFiles`  | array of strings                                                              | `[]`             | window | adapted       | Paths to documentation files for the configured definition files.                                                                   |
| `luau-lsp.types.disabledGlobals`     | array of strings                                                              | `[]`             | window | forwarded     | Removes complete globals or individual functions such as `table` or `table.clone`.                                                  |
| `luau-lsp.types.roblox`              | boolean                                                                       | `true`           | window | adapted       | Deprecated alias retained for automatic Roblox definitions. Prefer `platform.type`.                                                 |
| `luau-lsp.types.robloxSecurityLevel` | string: `None`, `LocalUserSecurity`, `PluginSecurity`, `RobloxScriptSecurity` | `PluginSecurity` | window | adapted       | Selects the downloaded Roblox API definition set.                                                                                   |

The wrapper adds managed Roblox definitions at the `@roblox` package when they
are enabled and no explicit Roblox definition was supplied. Definition and
documentation URLs are cached and refreshed by the wrapper. Explicit upstream
`--definitions` and `--docs` arguments remain available.

## Inlay hints and hover

| Setting                                                  | Type and allowed values           | Upstream default | Scope    | Compatibility | Description                                                                                |
| -------------------------------------------------------- | --------------------------------- | ---------------- | -------- | ------------- | ------------------------------------------------------------------------------------------ |
| `luau-lsp.inlayHints.parameterNames`                     | string: `none`, `literals`, `all` | `none`           | resource | forwarded     | Shows inlay hints for function parameter names.                                            |
| `luau-lsp.inlayHints.variableTypes`                      | boolean                           | `false`          | resource | forwarded     | Shows inlay hints for variable types.                                                      |
| `luau-lsp.inlayHints.parameterTypes`                     | boolean                           | `false`          | resource | forwarded     | Shows inlay hints for parameter types.                                                     |
| `luau-lsp.inlayHints.functionReturnTypes`                | boolean                           | `false`          | resource | forwarded     | Shows inlay hints for function return types.                                               |
| `luau-lsp.inlayHints.hideHintsForErrorTypes`             | boolean                           | `false`          | resource | forwarded     | Hides type hints that resolve to an error type.                                            |
| `luau-lsp.inlayHints.hideHintsForMatchingParameterNames` | boolean                           | `true`           | resource | forwarded     | Hides hints when the resolved variable name matches the parameter name.                    |
| `luau-lsp.inlayHints.typeHintMaxLength`                  | number, minimum `10`              | `50`             | resource | forwarded     | Maximum type-hint length before truncation.                                                |
| `luau-lsp.inlayHints.makeInsertable`                     | boolean                           | `true`           | resource | forwarded     | Allows type annotation hints to be inserted by clicking when the client supports the edit. |
| `luau-lsp.hover.enabled`                                 | boolean                           | `true`           | resource | forwarded     | Enables hover.                                                                             |
| `luau-lsp.hover.showTableKinds`                          | boolean                           | `false`          | resource | forwarded     | Shows table kinds in hover content.                                                        |
| `luau-lsp.hover.multilineFunctionDefinitions`            | boolean                           | `false`          | resource | forwarded     | Shows function definitions on multiple lines.                                              |
| `luau-lsp.hover.strictDatamodelTypes`                    | boolean                           | `true`           | resource | forwarded     | Uses strict DataModel types in hover display.                                              |
| `luau-lsp.hover.includeStringLength`                     | boolean                           | `true`           | resource | forwarded     | Shows string length when hovering over a string literal.                                   |

## Completion and signature help

| Setting                                                                   | Type and allowed values                            | Upstream default   | Scope    | Compatibility | Description                                                                                            |
| ------------------------------------------------------------------------- | -------------------------------------------------- | ------------------ | -------- | ------------- | ------------------------------------------------------------------------------------------------------ |
| `luau-lsp.completion.enabled`                                             | boolean                                            | `true`             | resource | forwarded     | Enables autocomplete.                                                                                  |
| `luau-lsp.autocompleteEnd`                                                | boolean                                            | `false`            | resource | partial       | Deprecated alias for `completion.autocompleteEnd`; portable completion edits depend on client support. |
| `luau-lsp.completion.autocompleteEnd`                                     | boolean                                            | `false`            | resource | partial       | Automatically inserts `end` when opening a block; private editor commands are unavailable.             |
| `luau-lsp.completion.addParentheses`                                      | boolean                                            | `true`             | resource | forwarded     | Adds parentheses after completing a function call.                                                     |
| `luau-lsp.completion.addTabstopAfterParentheses`                          | boolean                                            | `true`             | resource | forwarded     | Adds a tabstop after inserted call parentheses.                                                        |
| `luau-lsp.completion.fillCallArguments`                                   | boolean                                            | `true`             | resource | forwarded     | Fills parameter names in an autocompleted call. Requires `addParentheses`.                             |
| `luau-lsp.completion.showPropertiesOnMethodCall`                          | boolean                                            | `false`            | resource | forwarded     | Shows non-function properties for colon method calls such as `foo:bar`.                                |
| `luau-lsp.completion.showKeywords`                                        | boolean                                            | `true`             | resource | forwarded     | Shows Luau keywords during autocomplete.                                                               |
| `luau-lsp.completion.showAnonymousAutofilledFunction`                     | boolean                                            | `true`             | resource | forwarded     | Deprecated alias for `anonymousAutofilledFunction.enabled`.                                            |
| `luau-lsp.completion.anonymousAutofilledFunction.enabled`                 | boolean                                            | `true`             | resource | forwarded     | Shows generated anonymous callback function completions.                                               |
| `luau-lsp.completion.anonymousAutofilledFunction.addTypeAnnotations`      | boolean                                            | `true`             | resource | forwarded     | Adds type annotations to generated anonymous function snippets.                                        |
| `luau-lsp.completion.anonymousAutofilledFunction.addTabstopForParameters` | boolean                                            | `true`             | resource | forwarded     | Adds snippet tabstops to generated function parameters.                                                |
| `luau-lsp.completion.showDeprecatedItems`                                 | boolean                                            | `true`             | resource | forwarded     | Shows deprecated items in autocomplete suggestions.                                                    |
| `luau-lsp.completion.suggestImports`                                      | boolean                                            | `false`            | resource | forwarded     | Deprecated alias for `completion.imports.enabled`.                                                     |
| `luau-lsp.completion.imports.enabled`                                     | boolean                                            | `true`             | resource | forwarded     | Suggests automatic imports in completion items.                                                        |
| `luau-lsp.completion.imports.suggestServices`                             | boolean                                            | `true`             | resource | forwarded     | Suggests `GetService` completions when auto-importing.                                                 |
| `luau-lsp.completion.imports.includedServices`                            | array of strings                                   | `[]`               | resource | forwarded     | When non-empty, limits auto-imported services to this list.                                            |
| `luau-lsp.completion.imports.excludedServices`                            | array of strings                                   | `[]`               | resource | forwarded     | Excludes listed services from auto-import.                                                             |
| `luau-lsp.completion.imports.suggestRequires`                             | boolean                                            | `true`             | resource | forwarded     | Suggests module requires in autocomplete.                                                              |
| `luau-lsp.completion.imports.requireStyle`                                | string: `auto`, `alwaysRelative`, `alwaysAbsolute` | `auto`             | resource | forwarded     | Selects the style of autogenerated requires.                                                           |
| `luau-lsp.completion.imports.stringRequires.enabled`                      | boolean                                            | `false`            | resource | forwarded     | Uses string requires for Roblox auto-imports. Only applies when the platform is Roblox.                |
| `luau-lsp.completion.imports.separateGroupsWithLine`                      | boolean                                            | `false`            | resource | forwarded     | Separates services and requires with an empty line.                                                    |
| `luau-lsp.completion.imports.useConst`                                    | boolean                                            | `false`            | resource | forwarded     | Uses `const` instead of `local` for auto-imports.                                                      |
| `luau-lsp.completion.imports.ignoreGlobs`                                 | array of strings                                   | `["**/_Index/**"]` | resource | forwarded     | Excludes matching files from auto-import suggestions.                                                  |
| `luau-lsp.completion.enableFragmentAutocomplete`                          | boolean                                            | `true`             | resource | forwarded     | Enables fragment autocomplete for performance improvements.                                            |
| `luau-lsp.signatureHelp.enabled`                                          | boolean                                            | `true`             | resource | forwarded     | Enables signature help.                                                                                |

## Studio companion and legacy plugin settings

| Setting                                        | Type and allowed values | Upstream default | Scope  | Compatibility | Description                                                                                    |
| ---------------------------------------------- | ----------------------- | ---------------- | ------ | ------------- | ---------------------------------------------------------------------------------------------- |
| `luau-lsp.studioPlugin.enabled`                | boolean                 | `false`          | window | adapted       | Enables the wrapper-owned Roblox Studio companion bridge.                                      |
| `luau-lsp.studioPlugin.port`                   | number                  | `3667`           | window | adapted       | Loopback Studio companion port.                                                                |
| `luau-lsp.studioPlugin.maximumRequestBodySize` | string size             | `"3mb"`          | window | adapted       | Maximum Studio request body. Supports `b`, `kb`, `kib`, `mb`, `mib`, `gb`, and `gib` suffixes. |
| `luau-lsp.plugin.enabled`                      | boolean                 | `false`          | window | adapted       | Deprecated alias for `studioPlugin.enabled`.                                                   |
| `luau-lsp.plugin.port`                         | number                  | `3667`           | window | adapted       | Deprecated alias for `studioPlugin.port`.                                                      |
| `luau-lsp.plugin.maximumRequestBodySize`       | string size             | `"3mb"`          | window | adapted       | Deprecated alias for `studioPlugin.maximumRequestBodySize`.                                    |

The bridge binds only to `127.0.0.1`. The modern `studioPlugin` values take
precedence over legacy `plugin` values for port and body size. Either enabled
setting starts the bridge. The supported requests are `/full`, `/clear`, and
`/get-file-paths`.

## Require aliases, indexing, bytecode, and plugins

| Setting                                                | Type and allowed values | Upstream default | Scope    | Compatibility | Description                                                                                            |
| ------------------------------------------------------ | ----------------------- | ---------------- | -------- | ------------- | ------------------------------------------------------------------------------------------------------ |
| `luau-lsp.require.fileAliases`                         | object of string paths  | `{}`             | resource | forwarded     | Deprecated mapping of custom require string aliases to file paths. Prefer aliases in `.luaurc`.        |
| `luau-lsp.require.directoryAliases`                    | object of string paths  | `{}`             | resource | forwarded     | Deprecated mapping of require string prefixes to directories. Aliases should include trailing slashes. |
| `luau-lsp.require.useOriginalRequireByStringSemantics` | boolean                 | `false`          | resource | forwarded     | Deprecated switch for old `init.luau` require-by-string resolution.                                    |
| `luau-lsp.index.enabled`                               | boolean                 | `true`           | window   | forwarded     | Indexes workspace files for features such as Find All References and Rename.                           |
| `luau-lsp.index.maxFiles`                              | number                  | `10000`          | window   | forwarded     | Maximum number of indexed files. More files require more memory.                                       |
| `luau-lsp.bytecode.debugLevel`                         | number                  | `1`              | resource | forwarded     | `debugLevel` used for bytecode compilation.                                                            |
| `luau-lsp.bytecode.typeInfoLevel`                      | number                  | `1`              | resource | forwarded     | `typeInfoLevel` used for bytecode compilation.                                                         |
| `luau-lsp.bytecode.vectorLib`                          | string                  | `"Vector3"`      | resource | forwarded     | `vectorLib` used for bytecode compilation.                                                             |
| `luau-lsp.bytecode.vectorCtor`                         | string                  | `"new"`          | resource | forwarded     | `vectorCtor` used for bytecode compilation.                                                            |
| `luau-lsp.bytecode.vectorType`                         | string                  | `"Vector3"`      | resource | forwarded     | `vectorType` used for bytecode compilation.                                                            |
| `luau-lsp.plugins.enabled`                             | boolean                 | `false`          | resource | forwarded     | Enables Luau source transformation plugins before type checking.                                       |
| `luau-lsp.plugins.paths`                               | array of strings        | `[]`             | resource | forwarded     | Plugin script paths, executed in order.                                                                |
| `luau-lsp.plugins.timeoutMs`                           | number                  | `5000`           | resource | forwarded     | Plugin execution timeout in milliseconds.                                                              |
| `luau-lsp.plugins.fileSystem.enabled`                  | boolean                 | `false`          | resource | forwarded     | Allows plugins to read files within the workspace.                                                     |

## Examples

Disable automatic sourcemaps and use a custom Roblox security level:

```json
{
  "settings": {
    "luau-lsp": {
      "sourcemap": {
        "enabled": false
      },
      "types": {
        "robloxSecurityLevel": "LocalUserSecurity"
      }
    }
  }
}
```

Enable the Studio companion and configure the new solver through Neovim or
another LSP client:

```json
{
  "settings": {
    "luau-lsp.studioPlugin.enabled": true,
    "luau-lsp.fflags.enableNewSolver": true,
    "luau-lsp.fflags.override": {
      "LuauSolverV2": "true"
    }
  }
}
```

## Neovim

`luau-lsp-roblox` speaks standard LSP over stdio, so it can be used with
Neovim's built-in client. The example below targets Neovim 0.11 or newer.

### Managed Roblox mode

Put this in `init.lua` or a Lua module loaded by it:

```lua
vim.lsp.config("luau_lsp", {
  cmd = { "luau-lsp", "lsp" },
  filetypes = { "luau" },
  root_markers = { "default.project.json", ".git" },
  settings = {
    ["luau-lsp"] = {
      sourcemap = {
        enabled = true,
        autogenerate = true,
        rojoProjectFile = "default.project.json",
        sourcemapFile = "sourcemap.json",
        includeNonScripts = true,
      },
      types = {
        robloxSecurityLevel = "PluginSecurity",
      },
      fflags = {
        enableNewSolver = true,
      },
    },
  },
})

vim.lsp.enable("luau_lsp")
```

Managed Roblox mode is already the default. The wrapper downloads or refreshes
the Roblox definitions, synchronizes compatible published Roblox Luau FFlags,
and generates a Rojo sourcemap when the project contains
`default.project.json`. The explicit settings above show the nesting expected
by Neovim; they are not required for the defaults.

Neovim sends its `settings` table through the LSP configuration flow. The
wrapper accepts the same table as dotted keys, a nested `luau-lsp` object, or a
section value.

### Settings variants

Use a different security definition set or enable the Studio companion through
the same settings table:

```lua
vim.lsp.config("luau_lsp", {
  cmd = { "luau-lsp", "lsp" },
  filetypes = { "luau" },
  root_markers = { "default.project.json", ".git" },
  settings = {
    ["luau-lsp"] = {
      types = {
        robloxSecurityLevel = "LocalUserSecurity",
      },
      studioPlugin = {
        enabled = true,
      },
    },
  },
})
```

The Studio companion listens only on `127.0.0.1`; its default port is `3667`.
The equivalent CLI flags are available for one-off overrides, but are not
needed for normal Neovim configuration.

### Standard mode

To use the upstream server without managed Roblox behavior, select standard
mode in the command:

```lua
vim.lsp.config("luau_lsp_standard", {
  cmd = { "luau-lsp", "lsp", "--platform", "standard" },
  filetypes = { "luau" },
  root_markers = { ".git" },
})

vim.lsp.enable("luau_lsp_standard")
```

Wrapper-owned settings apply only to managed Roblox mode. Standard mode passes
upstream arguments and configuration through transparently.

### JSON settings files

For settings shared with other editors, point the command at a JSON file:

```lua
vim.lsp.config("luau_lsp", {
  cmd = {
    "luau-lsp",
    "lsp",
    "--wrapper-settings",
    vim.fn.getcwd() .. "/luau-lsp.json",
  },
  filetypes = { "luau" },
  root_markers = { "default.project.json", ".git" },
})
```

The file may contain either a direct settings object or a top-level `settings`
object. For example:

```json
{
  "settings": {
    "luau-lsp.sourcemap.rojoProjectFile": "game.project.json",
    "luau-lsp.fflags.override": {
      "LuauSolverV2": "true"
    }
  }
}
```

Relative definition and documentation paths are resolved against the workspace
when possible. If a workspace contains multiple Rojo roots, pass an explicit
upstream `--sourcemap` argument in `cmd`.

### Troubleshooting

- Confirm that `luau-lsp` and `rojo` are on Neovim's `PATH`.
- Use `:LspInfo` to confirm that the client started from the expected project
  root.
- Run `luau-lsp --wrapper-help` in a terminal to inspect wrapper options.
- If a setting is changed after initialization and the wrapper logs that a
  restart is required, restart the LSP client so startup-owned resources can be
  rebuilt.

## Helix

Helix's language bundle already defines the `luau` language, including its file
types and project roots. Keep that existing `[[language]]` entry; only define or
extend the language-server entry when needed:

```toml
[language-server.luau]
command = "luau-lsp"
args = ["lsp"]
```

Helix passes the language-server `config` table as initialization options. The
wrapper accepts embedded `luau-lsp` settings there, so configure the wrapper
without adding CLI flags:

```toml
[language-server.luau.config."luau-lsp".sourcemap]
enabled = true
autogenerate = true
rojoProjectFile = "default.project.json"
sourcemapFile = "sourcemap.json"
includeNonScripts = true

[language-server.luau.config."luau-lsp".types]
definitionFiles = { "@roblox" = "types.d.luau" }
documentationFiles = ["api-docs.json"]
robloxSecurityLevel = "PluginSecurity"

[language-server.luau.config."luau-lsp".fflags]
enableNewSolver = true

[language-server.luau.config."luau-lsp".completion.imports.stringRequires]
enabled = true

[language-server.luau.config."luau-lsp".completion.imports]
useConst = true
```

If your server uses a different Helix ID, replace `luau` in the table paths;
the embedded settings namespace remains `luau-lsp`.

Managed Roblox mode is already the default. The settings above are shown for
shape and can be omitted when the managed defaults are sufficient. For a JSON
settings file shared with other editors, add `--wrapper-settings` and its path
to the server's `args` array. See [Configuration sources](#configuration-sources)
for the accepted file shapes.

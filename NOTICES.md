# Notices

Release archives contain two independently versioned programs:

- `luau-lsp`, the Rust wrapper from this repository, MIT;
- `luau-lsp-server`, the exact upstream `JohnnyMorganz/luau-lsp` release pinned
  in `upstream/manifest.json`, MIT.

The release workflow includes both license texts in every archive. Runtime Roblox
API definition and documentation files are fetched from the endpoints pinned in
`upstream/manifest.json` and fall back to the copies packaged at release
time.

//! Editor-neutral Roblox environment management for the bundled `luau-lsp` server.

mod args;
mod assets;
mod config;
mod lsp;
mod process;
mod runtime;
mod services;

pub use runtime::run;

type Error = Box<dyn std::error::Error + Send + Sync>;
type Result<T> = std::result::Result<T, Error>;

fn error(message: impl Into<String>) -> Error {
    std::io::Error::other(message.into()).into()
}

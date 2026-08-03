//! Public `luau-lsp` executable entry point.

use std::process::ExitCode;

fn main() -> ExitCode {
    match luau_lsp_roblox::run() {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            use std::io::Write as _;

            let _result = writeln!(std::io::stderr().lock(), "luau-lsp: {error}");
            ExitCode::FAILURE
        }
    }
}

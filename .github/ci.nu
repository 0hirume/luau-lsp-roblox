def fail [message: string]: nothing -> error {
    error make {
        msg: $message
        label: {
            text: $message
            span: (metadata $message).span
        }
    }
}

def main []: nothing -> nothing {
    let pattern = '#\s*\[\s*allow|clippy::allow|RUSTFLAGS=.*-A'

    let result = (
        run-external
            rg
            "-n"
            $pattern
            Cargo.toml
            src
        | complete
    )

    if $result.exit_code == 0 {
        print ($result.stdout | str trim)
        fail "lint suppressions are prohibited"
    }

    if $result.exit_code != 1 {
        fail $"searching for lint suppressions failed: ($result.stderr | str trim)"
    }
}

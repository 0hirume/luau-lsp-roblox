def main []: nothing -> nothing {
    let pattern = '#\s*\[\s*allow|clippy::allow|RUSTFLAGS=.*-A'
    let files = [Cargo.toml] | append (glob "src/**/*.rs")
    let matches = (
        $files
        | each {|path|
            let content = try {
                open --raw $path
            } catch {
                let message = $"reading ($path) failed"
                error make {
                    msg: $message
                    label: {
                        text: $message
                        span: (metadata $path).span
                    }
                }
            }
            $content
            | lines
            | enumerate
            | where item =~ $pattern
            | each {|line| $"($path):($line.index + 1):($line.item)" }
        }
        | flatten
    )

    if ($matches | is-not-empty) {
        print ($matches | str join (char newline))
        error make {
            msg: "lint suppressions are prohibited"
            label: {
                text: "lint suppressions are prohibited"
                span: (metadata $pattern).span
            }
        }
    }
}

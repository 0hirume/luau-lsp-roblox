def fail [message: string]: nothing -> error {
    error make {
        msg: $message
        label: {
            text: $message
            span: (metadata $message).span
        }
    }
}

def require [condition: bool, message: string]: nothing -> nothing {
    if not $condition {
        fail $message
    }
}

def checked [program: string, ...arguments: string]: nothing -> nothing {
    run-external $program ...$arguments

    let exit_code = $env.LAST_EXIT_CODE

    if $exit_code != 0 {
        fail $"($program) failed with exit code ($exit_code)"
    }
}

def capture [program: string, ...arguments: string]: nothing -> string {
    let result = run-external $program ...$arguments | complete

    if $result.exit_code != 0 {
        fail $"($program) failed: ($result.stderr | str trim)"
    }

    $result.stdout | str trim
}

def download [url: string, path: string]: nothing -> nothing {
    checked curl "--fail" "--location" "--retry" "3" $url "--output" $path
}

def main []: nothing -> nothing {
    try {
        let target = $env.CARGO_BUILD_TARGET
        let platform = match $target {
            "x86_64-pc-windows-msvc" => {
                os: windows
                arch: x86_64
                upstream: windows-x86_64
                suffix: .exe
            }
            "x86_64-unknown-linux-gnu" => {
                os: linux
                arch: x86_64
                upstream: linux-x86_64
                suffix: ""
            }
            "aarch64-unknown-linux-gnu" => {
                os: linux
                arch: aarch64
                upstream: linux-aarch64
                suffix: ""
            }
            "x86_64-apple-darwin" => {
                os: macos
                arch: x86_64
                upstream: macos-universal
                suffix: ""
            }
            "aarch64-apple-darwin" => {
                os: macos
                arch: aarch64
                upstream: macos-universal
                suffix: ""
            }
            _ => { fail $"unsupported release target: ($target)" }
        }
        let tag = $env | get --optional RELEASE_TAG | default $env.GITHUB_REF_NAME
        require ($tag | str starts-with v) $"release tag must start with v: ($tag)"
        let version = $tag | str substring 1..
        let manifest = open upstream/manifest.json
        let asset = $manifest.release_assets | get --optional $platform.upstream
        require ($asset != null) $"upstream release has no asset for ($platform.upstream)"
        let server = $"luau-lsp-server($platform.suffix)"
        let archive = $"luau-lsp-v($version)-($platform.os)-($platform.arch)"

        mkdir bundle upstream-download
        let upstream_url = $"https://github.com/($manifest.repository)/releases/download/($manifest.version)/($asset.name)"
        download $upstream_url upstream.zip
        let actual_size = ls upstream.zip | get size | first | into int
        require ($actual_size == $asset.size) $"upstream archive size mismatch: expected ($asset.size), got ($actual_size)"
        let actual_hash = open --raw upstream.zip | hash sha256
        require ($actual_hash == $asset.sha256) $"upstream archive digest mismatch: expected ($asset.sha256), got ($actual_hash)"

        if $platform.os == windows {
            checked 7z x upstream.zip "-oupstream-download"
        } else {
            checked unzip "-q" upstream.zip "-d" upstream-download
        }

        let matches = glob $"upstream-download/**/luau-lsp($platform.suffix)"
        require (($matches | length) == 1) $"expected exactly one upstream executable, found ($matches | length)"
        let upstream_binary = $matches | first
        let server_path = [bundle $server] | path join
        cp $upstream_binary $server_path

        if $platform.os != windows {
            checked chmod "755" $server_path
        }

        let upstream_version = capture $server_path "--version"
        require (($upstream_version | str index-of $manifest.version) >= 0) $"upstream executable reported an unexpected version: ($upstream_version)"
        let flags = capture $server_path "--show-flags"
        require ($flags =~ '=') "upstream executable returned no fast flags"

        cp LICENSE bundle/LICENSE-wrapper
        download $"https://raw.githubusercontent.com/($manifest.repository)/($manifest.commit)/LICENSE.md" bundle/LICENSE-upstream

        let include = [
            $"bundle/($server)"
            bundle/LICENSE-wrapper
            bundle/LICENSE-upstream
        ] | str join ,
        let output = [
            $"archive=($archive)"
            $"include=($include)"
        ] | str join (char newline)
        ($output + (char newline)) | save --append $env.GITHUB_OUTPUT
    } catch {|error| fail $error.msg }
}

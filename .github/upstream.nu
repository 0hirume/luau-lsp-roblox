const MANIFEST = "upstream/manifest.json"
const SCHEMA = "upstream/schema.json"
const COMPATIBILITY = "upstream/compatibility.json"

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

def api [...arguments: string]: nothing -> record {
    let result = run-external gh api ...$arguments | complete

    if $result.exit_code != 0 {
        fail ($result.stderr | str trim)
    }

    try {
        $result.stdout | from json
    } catch {|error| fail $error.msg }
}

def read-json [path: path]: nothing -> record {
    try {
        open $path
    } catch {|error| fail $error.msg }
}

def write-json [path: path, value: record]: nothing -> nothing {
    try {
        (($value | to json --indent 2) + (char newline)) | save --force $path
    } catch {|error| fail $error.msg }
}

def pinned-asset [key: string, pattern: string]: table -> record {
    let matches = $in | where name =~ $pattern
    require (($matches | length) == 1) $"expected one ($key) release asset"
    let asset = $matches | first
    let raw_digest = $asset | get --optional digest
    require ($raw_digest != null) $"release asset ($asset.name) has no digest"
    let digest = $raw_digest | str replace sha256: ""
    require ($digest =~ '^[0-9a-f]{64}$') $"release asset ($asset.name) has no valid digest"
    require ($asset.size > 0) $"release asset ($asset.name) is empty"

    {name: $asset.name, sha256: $digest, size: $asset.size}
}

# Update the pinned upstream release and editor settings.
def main [
    version?: string # Specific stable version; defaults to GitHub's latest release.
]: nothing -> nothing {
    let previous = read-json $MANIFEST
    let repository = $previous.repository
    let release_path = if $version == null {
        $"repos/($repository)/releases/latest"
    } else {
        require ($version =~ ^\d+\.\d+\.\d+$) $"invalid stable version: ($version)"
        $"repos/($repository)/releases/tags/($version)"
    }
    let release = api $release_path
    let selected = $release.tag_name
    require ($selected =~ ^\d+\.\d+\.\d+$) $"release tag is not stable SemVer: ($selected)"
    require (not $release.draft) $"release ($selected) is a draft"
    require (not $release.prerelease) $"release ($selected) is a prerelease"

    let commit = api $"repos/($repository)/commits/($selected)" | get sha
    require ($commit =~ '^[0-9a-f]{40}$') $"release ($selected) did not resolve to a commit"
    let package = (
        api
            "-H"
            "Accept: application/vnd.github.raw+json"
            $"repos/($repository)/contents/editors/code/package.json?ref=($commit)"
    )
    require ($package.version == $selected) $"editor package version does not match ($selected)"

    let old_schema = read-json $SCHEMA
    let old_compatibility = read-json $COMPATIBILITY
    let old_settings = $old_schema.settings
    let settings = $package.contributes.configuration.properties
    let old_keys = $old_settings | columns
    let new_keys = $settings | columns
    let changed = (
        $new_keys
        | where (
            ($it in $old_keys) and (
                ($settings | get --optional $it) != ($old_settings | get --optional $it)
            )
        )
    )
    let added = $new_keys | where $it not-in $old_keys
    let removed = $old_keys | where $it not-in $new_keys
    let preserved = (
        $old_compatibility.settings
        | columns
        | where ($it in $new_keys) and ($it not-in $changed)
    )
    let compatibility_settings = $old_compatibility.settings | select ...$preserved

    let manifest = {
        repository: $repository
        version: $selected
        commit: $commit
        release_assets: {
            windows-x86_64: (
                $release.assets | pinned-asset windows-x86_64 ^luau-lsp-win64[.]zip$
            )
            linux-x86_64: (
                $release.assets | pinned-asset linux-x86_64 ^luau-lsp-linux-x86_64[.]zip$
            )
            linux-aarch64: (
                $release.assets | pinned-asset linux-aarch64 ^luau-lsp-linux-arm64[.]zip$
            )
            macos-universal: (
                $release.assets | pinned-asset macos-universal ^luau-lsp-macos[.]zip$
            )
        }
        roblox: $previous.roblox
        studio: $previous.studio
    }
    let schema = {}
    | insert upstream_version $selected
    | insert upstream_commit $commit
    | insert settings $settings
    let compatibility = {}
    | insert upstream_version $selected
    | insert upstream_commit $commit
    | insert settings $compatibility_settings

    write-json $MANIFEST $manifest
    write-json $SCHEMA $schema
    write-json $COMPATIBILITY $compatibility

    print $"updated upstream to ($selected) at ($commit)"
    print $"settings: ($new_keys | length); preserved classifications: ($preserved | length)"

    if not (($added | is-empty) and ($changed | is-empty) and ($removed | is-empty)) {
        print $"compatibility review required; added=($added), changed=($changed), removed=($removed)"
    }
}

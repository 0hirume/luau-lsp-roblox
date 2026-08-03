const DEFAULT_REPOSITORY = "JohnnyMorganz/luau-lsp"
const MANIFEST_PATH = "upstream/manifest.json"
const SCHEMA_PATH = "upstream/schema.json"
const COMPATIBILITY_PATH = "upstream/compatibility.json"
const ASSET_KEYS = [
    "windows-x86_64"
    "linux-x86_64"
    "linux-aarch64"
    "macos-universal"
]

def fail [message: string] {
    error make { msg: $message }
}

def require [condition: bool, message: string] {
    if not $condition {
        fail $message
    }
}

def capture [program: string, arguments: list<string>] {
    let result = (run-external $program ...$arguments | complete)
    if $result.exit_code != 0 {
        let detail = ($result.stderr | str trim)
        fail $"($program) failed with exit code ($result.exit_code): ($detail)"
    }
    $result.stdout
}

def save-json [path: string, value: any] {
    let contents = $"($value | to json --indent 2)(char newline)"
    $contents | save --force $path
}

def legacy-path [kind: string] {
    let files = (glob $"upstream/($kind)-*.json" | sort)
    if ($files | is-empty) {
        null
    } else {
        $files | last | into string
    }
}

def optional-json [path: string, legacy: string] {
    if ($path | path exists) {
        open $path
    } else {
        let fallback = (legacy-path $legacy)
        if $fallback == null {
            null
        } else {
            open $fallback
        }
    }
}

def exact-tag-commit [repository: string, version: string] {
    require ($version =~ '^\d+\.\d+\.\d+$') $"invalid stable version: ($version)"
    let remote = $"https://github.com/($repository).git"
    let output = (capture git [
        "ls-remote"
        "--tags"
        $remote
        $"refs/tags/($version)"
        $"refs/tags/($version)^{}"
    ])
    let refs = ($output | lines | each {|line|
        let fields = ($line | split row (char tab))
        if ($fields | length) == 2 {
            { commit: $fields.0, reference: $fields.1 }
        }
    } | compact)
    require (not ($refs | is-empty)) $"upstream tag ($version) does not exist"
    let peeled = ($refs | where {|entry| $entry.reference | str ends-with '^{}'})
    let selected = if ($peeled | is-empty) { $refs | first } else { $peeled | first }
    require ($selected.commit =~ '^[0-9a-f]{40}$') $"tag ($version) did not resolve to a commit"
    $selected.commit
}

def latest-version [repository: string] {
    let remote = $"https://github.com/($repository).git"
    let output = (capture git ["ls-remote" "--tags" "--refs" $remote])
    let versions = ($output | lines | each {|line|
        let fields = ($line | split row (char tab))
        if ($fields | length) == 2 {
            $fields.1 | str replace 'refs/tags/' ''
        }
    } | compact | where {|version| $version =~ '^\d+\.\d+\.\d+$'} | each {|version|
        let parts = ($version | split row '.' | each {|part| $part | into int})
        { version: $version, major: $parts.0, minor: $parts.1, patch: $parts.2 }
    } | sort-by major minor patch)
    require (not ($versions | is-empty)) "upstream has no stable semantic-version tags"
    $versions | last | get version
}

def release [repository: string, version: string] {
    let output = (capture gh ["api" $"repos/($repository)/releases/tags/($version)"])
    let release = ($output | from json)
    require ($release.tag_name == $version) $"release tag does not match ($version)"
    require (not $release.draft) $"release ($version) is still a draft"
    require (not $release.prerelease) $"release ($version) is a prerelease"
    $release
}

def release-asset [assets: table, key: string, pattern: string] {
    let matches = ($assets | where {|asset| $asset.name =~ $pattern})
    require (($matches | length) == 1) $"expected exactly one ($key) release asset, found ($matches | length)"
    let asset = ($matches | first)
    require ($asset.digest != null) $"release asset ($asset.name) has no digest"
    let digest = ($asset.digest | str replace 'sha256:' '')
    require ($digest =~ '^[0-9a-f]{64}$') $"release asset ($asset.name) has an invalid SHA-256 digest"
    require ($asset.size > 0) $"release asset ($asset.name) is empty"
    {
        name: $asset.name
        sha256: $digest
        size: $asset.size
    }
}

def upstream-package [repository: string, commit: string] {
    let output = (capture gh [
        "api"
        "-H"
        "Accept: application/vnd.github.raw+json"
        $"repos/($repository)/contents/editors/code/package.json?ref=($commit)"
    ])
    $output | from json
}

def changed-settings [old_schema: any, settings: record] {
    if $old_schema == null {
        return []
    }
    let old_settings = $old_schema.settings
    if not (($old_settings | describe) | str starts-with 'record') {
        return []
    }
    let old_keys = ($old_settings | columns)
    $settings | columns | where {|key|
        ($key in $old_keys) and (($settings | get $key) != ($old_settings | get $key))
    }
}

def preserved-compatibility [old_compatibility: any, settings: record, changed: list<string>] {
    if $old_compatibility == null {
        return {}
    }
    let keys = ($settings | columns)
    $old_compatibility.settings
        | transpose key value
        | where {|entry| ($entry.key in $keys) and ($entry.key not-in $changed)}
        | reduce --fold {} {|entry, result| $result | insert $entry.key $entry.value}
}

def validate-local [allow_pending: bool] {
    for path in [$MANIFEST_PATH $SCHEMA_PATH $COMPATIBILITY_PATH] {
        require ($path | path exists) $"missing ($path)"
    }

    let manifest = (open $MANIFEST_PATH)
    let schema = (open $SCHEMA_PATH)
    let compatibility = (open $COMPATIBILITY_PATH)
    require ($manifest.version =~ '^\d+\.\d+\.\d+$') "manifest version is not stable SemVer"
    require ($manifest.commit =~ '^[0-9a-f]{40}$') "manifest commit is invalid"
    require ($schema.upstream_version == $manifest.version) "schema version does not match manifest"
    require ($schema.upstream_commit == $manifest.commit) "schema commit does not match manifest"
    require ($compatibility.upstream_version == $manifest.version) "compatibility version does not match manifest"
    require ($compatibility.upstream_commit == $manifest.commit) "compatibility commit does not match manifest"

    let asset_keys = ($manifest.release_assets | columns | sort)
    require ($asset_keys == ($ASSET_KEYS | sort)) "manifest release asset keys are incomplete"
    for key in $ASSET_KEYS {
        let asset = ($manifest.release_assets | get $key)
        require (($asset.name | str length) > 0) $"release asset ($key) has no name"
        require ($asset.sha256 =~ '^[0-9a-f]{64}$') $"release asset ($key) has an invalid SHA-256 digest"
        require ($asset.size > 0) $"release asset ($key) is empty"
    }

    let schema_keys = ($schema.settings | columns | sort)
    let compatibility_keys = ($compatibility.settings | columns | sort)
    require (not ($schema_keys | is-empty)) "upstream schema has no settings"
    if not $allow_pending {
        let missing = ($schema_keys | where {|key| $key not-in $compatibility_keys})
        let removed = ($compatibility_keys | where {|key| $key not-in $schema_keys})
        require (($missing | is-empty) and ($removed | is-empty)) $"compatibility review required; missing=($missing), removed=($removed)"
    }
}

def update [repository: string, requested_version: string] {
    let version = if ($requested_version | is-empty) {
        latest-version $repository
    } else {
        $requested_version
    }
    let commit = (exact-tag-commit $repository $version)
    let release = (release $repository $version)
    let package = (upstream-package $repository $commit)
    require ($package.version == $version) $"editor package version ($package.version) does not match tag ($version)"

    let old_manifest = (optional-json $MANIFEST_PATH "upstream")
    let old_schema = (optional-json $SCHEMA_PATH "schema")
    let old_compatibility = (optional-json $COMPATIBILITY_PATH "compatibility")
    let settings = $package.contributes.configuration.properties
    let changed = (changed-settings $old_schema $settings)
    let old_keys = if $old_schema == null {
        []
    } else if (($old_schema.settings | describe) | str starts-with 'record') {
        $old_schema.settings | columns
    } else {
        $old_schema.settings
    }
    let new_keys = ($settings | columns)
    let added = ($new_keys | where {|key| $key not-in $old_keys})
    let removed = ($old_keys | where {|key| $key not-in $new_keys})
    let compatibility_settings = (preserved-compatibility $old_compatibility $settings $changed)

    let manifest = {
        repository: $repository
        version: $version
        commit: $commit
        release_assets: {
            "windows-x86_64": (release-asset $release.assets "windows-x86_64" '^luau-lsp-win64[.]zip$')
            "linux-x86_64": (release-asset $release.assets "linux-x86_64" '^luau-lsp-linux-x86_64[.]zip$')
            "linux-aarch64": (release-asset $release.assets "linux-aarch64" '^luau-lsp-linux-arm64[.]zip$')
            "macos-universal": (release-asset $release.assets "macos-universal" '^luau-lsp-macos[.]zip$')
        }
        roblox: (if $old_manifest == null {
            {
                definitions: "https://luau-lsp.pages.dev/type-definitions/globalTypes."
                documentation: "https://luau-lsp.pages.dev/api-docs/en-us.json"
                fflags: "https://clientsettingscdn.roblox.com/v1/settings/application?applicationName=PCStudioApp"
            }
        } else {
            $old_manifest.roblox
        })
        studio: (if $old_manifest == null {
            {
                default_port: 3667
                default_maximum_request_body_size: "3mb"
                plugin: "https://www.roblox.com/library/10913122509/Luau-Language-Server-Companion"
            }
        } else {
            $old_manifest.studio
        })
    }
    let schema = {
        upstream_version: $version
        upstream_commit: $commit
        settings: $settings
    }
    let compatibility = {
        upstream_version: $version
        upstream_commit: $commit
        settings: $compatibility_settings
    }

    save-json $MANIFEST_PATH $manifest
    save-json $SCHEMA_PATH $schema
    save-json $COMPATIBILITY_PATH $compatibility
    validate-local true

    print $"updated upstream to ($version) at ($commit)"
    print $"schema settings: ($new_keys | length); preserved classifications: ($compatibility_settings | columns | length)"
    if not (($added | is-empty) and ($changed | is-empty) and ($removed | is-empty)) {
        print $"compatibility review required; added=($added), changed=($changed), removed=($removed)"
    }
}

def main [
    version?: string
    --check
    --repository: string = $DEFAULT_REPOSITORY
] {
    if $check {
        validate-local false
        print "upstream metadata is internally consistent"
    } else {
        update $repository ($version | default "")
    }
}

use std/assert

const BRANCH = "automation/upstream"
const TITLE = "chore: update upstream"
const BODY = "Pins the exact upstream release tag, schema, asset names, sizes, and SHA-256 digests. CI remains blocked until every added or changed setting is explicitly classified."
const EMAIL = "41898282+github-actions[bot]@users.noreply.github.com"

def checked [program: string, ...arguments: string]: nothing -> nothing {
    run-external $program ...$arguments
    assert ($env.LAST_EXIT_CODE == 0) $"($program) failed"
}

def capture [program: string, ...arguments: string]: nothing -> string {
    let result = run-external $program ...$arguments | complete

    assert ($result.exit_code == 0) $"($program) failed: ($result.stderr | str trim)"
    $result.stdout | str trim
}

# Commit a generated upstream update and open or refresh its pull request.
def main []: nothing -> nothing {
    if (capture git status "--short" "--" upstream | is-empty) {
        print "upstream is already current"
        return
    }

    checked gh auth setup-git
    checked git config user.name "github-actions[bot]"
    checked git config user.email $EMAIL
    checked git switch "--force-create" $BRANCH
    checked git add upstream
    checked git commit "--message" $TITLE

    let reference = $"refs/heads/($BRANCH)"
    let remote = capture git ls-remote "--heads" origin $reference

    if ($remote | is-empty) {
        checked git push origin $"HEAD:($reference)"
    } else {
        let commit = $remote | split row (char tab) | first
        (checked
            git
            push
            $"--force-with-lease=($reference):($commit)"
            origin
            $"HEAD:($reference)"
        )
    }

    let query = [
        "--base" $env.BASE_BRANCH
        "--head" $BRANCH
        "--state" open
        "--json" number
        "--jq" ".[0].number"
    ]
    let number = capture gh pr list ...$query

    if ($number | is-empty) {
        let details = [
            "--base" $env.BASE_BRANCH
            "--head" $BRANCH
            "--title" $TITLE
            "--body" $BODY
        ]
        checked gh pr create ...$details
    } else {
        print $"updated pull request #($number)"
    }
}

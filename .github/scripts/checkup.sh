#!/bin/bash
# Backticks in single quotes below are Markdown code spans, not substitutions.
# shellcheck disable=SC2016
# The monthly checkup (.github/workflows/checkup.yml). Also runs by hand from
# the repo root: .github/scripts/checkup.sh [report.md]
#
# Each check appends a Markdown section to the report only when something needs
# a look, so an empty report means all clear. Needs cargo, cargo-audit, curl, jq
# and a logged-in gh (for the upstream commit list). The working tree is left
# as it was: Cargo.lock is put back after trying the dependency updates.
set -uo pipefail

REPORT=${1:-checkup.md}
UPSTREAM_REPO=immich-app/immich
UPSTREAM_PACKAGE=@immich/cli
# Where the CLI lives in the upstream monorepo (packages/cli since May 2026).
UPSTREAM_PATH=packages/cli
LOG_LINES=40

LOG_DIR=$(mktemp -d)
trap 'rm -rf "$LOG_DIR"' EXIT

# run_check <name> <command...>: runs the command with its output kept in
# $LOG_DIR/<name>.log, and succeeds or fails like it.
run_check() {
    local name=$1
    shift
    "$@" >"$LOG_DIR/$name.log" 2>&1
}

# log_block <name> <summary>: the last lines of a check's output, folded.
log_block() {
    printf '<details><summary>%s</summary>\n\n```\n' "$2"
    tail -n "$LOG_LINES" "$LOG_DIR/$1.log"
    printf '```\n\n</details>\n\n'
}

package_field() {
    cargo metadata --no-deps --format-version 1 | jq -r ".packages[0].$1"
}

# Formatting, lints and tests with the locked dependencies, on whatever Rust
# is installed (the newest stable in the workflow): new lints and toolchain
# changes show up here first.
check_toolchain() {
    local -a names=(fmt clippy test)
    local -A commands=(
        [fmt]="cargo fmt --check -- --color never"
        [clippy]="cargo clippy --locked --all-targets -- -D warnings"
        [test]="cargo test --locked"
    )
    local name failed=() cmd
    for name in "${names[@]}"; do
        read -ra cmd <<<"${commands[$name]}"
        run_check "$name" "${cmd[@]}" || failed+=("$name")
    done
    ((${#failed[@]})) || return 0
    {
        printf '## Build on %s\n\n' "$(rustc --version)"
        printf 'These fail with the locked dependencies:\n\n'
        for name in "${failed[@]}"; do
            log_block "$name" "<code>${commands[$name]}</code>"
        done
    } >>"$REPORT"
}

# RustSec advisories against Cargo.lock, including yanked and unmaintained
# crates.
check_advisories() {
    run_check audit cargo audit --deny warnings && return 0
    {
        printf '## Security advisories\n\n'
        printf '`cargo audit` reports advisories, or yanked or unmaintained crates, in Cargo.lock:\n\n'
        log_block audit "<code>cargo audit --deny warnings</code>"
    } >>"$REPORT"
}

# Newer versions of the dependencies: the ones Cargo.toml already allows are
# tried against the tests; newer major versions are listed for the direct
# dependencies only (the others follow whatever their parents require).
check_dependencies() {
    cp Cargo.lock "$LOG_DIR/Cargo.lock"
    if ! run_check update cargo update --verbose; then
        {
            printf '## Dependencies\n\n'
            log_block update "<code>cargo update</code> failed"
        } >>"$REPORT"
        return
    fi

    local direct updated behind tests=""
    direct=$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[0].dependencies[].name')
    updated=$(grep -E '^\s+(Updating \S+ v\S+ -> v|Downgrading \S+ v\S+ -> v|Adding \S+ v|Removing \S+ v)' "$LOG_DIR/update.log" |
        sed -E 's/^\s+/- /')
    # "Unchanged ureq v3.1.0 (available: v4.0.0)"
    behind=$(grep -E '^\s+Unchanged ' "$LOG_DIR/update.log" |
        while read -r _ name version _ latest; do
            grep -qxF "$name" <<<"$direct" && echo "- $name $version, newest ${latest%)}"
        done)

    if [[ -n "$updated" ]]; then
        if run_check updated-test cargo test; then
            tests='The tests pass with them: run `cargo update` and commit Cargo.lock.'
        else
            tests=$(printf 'The tests fail with them:\n\n%s' "$(log_block updated-test '<code>cargo test</code> after <code>cargo update</code>')")
        fi
    fi
    cp "$LOG_DIR/Cargo.lock" Cargo.lock

    [[ -n "$updated$behind" ]] || return 0
    {
        printf '## Dependencies\n\n'
        if [[ -n "$updated" ]]; then
            printf 'Updates within the versions Cargo.toml allows:\n\n%s\n\n%s\n\n' "$updated" "$tests"
        fi
        if [[ -n "$behind" ]]; then
            printf 'New major versions (raise them in Cargo.toml):\n\n%s\n\n' "$behind"
        fi
    } >>"$REPORT"
}

# A newer @immich/cli than the one immich-rs matches (its version in
# Cargo.toml), with the upstream commits under the CLI's folder since then.
check_upstream() {
    local ported latest base base_sha since commits
    ported=$(package_field version)
    if ! latest=$(curl -fsS "https://registry.npmjs.org/$UPSTREAM_PACKAGE/latest" | jq -er .version); then
        printf '## Upstream\n\nCould not read the newest %s version from npm.\n\n' "$UPSTREAM_PACKAGE" >>"$REPORT"
        return
    fi
    [[ "$latest" == "$ported" ]] && return 0

    if ! base=$(gh api "repos/$UPSTREAM_REPO/commits/v$ported" --jq '.sha + " " + .commit.committer.date') ||
        ! read -r base_sha since <<<"$base"; then
        printf '## Upstream\n\n%s %s is out, but the tag v%s was not found in %s.\n\n' \
            "$UPSTREAM_PACKAGE" "$latest" "$ported" "$UPSTREAM_REPO" >>"$REPORT"
        return
    fi
    # Version bumps and dependency chores are left out. @ and # get a zero-width
    # space so the issue neither mentions people nor links upstream pull
    # requests (which would add a reference to each of them).
    commits=$(gh api --paginate "repos/$UPSTREAM_REPO/commits?path=$UPSTREAM_PATH&sha=v$latest&since=$since&per_page=100" |
        jq -r --arg base "$base_sha" '.[]
            | select(.sha != $base)
            | (.commit.message | split("\n")[0]) as $title
            | select(($title | startswith("chore: version")) or ($title | startswith("chore(deps)")) | not)
            | "- [`\(.sha[0:7])`](\(.html_url)) \($title | gsub("@"; "@​") | gsub("#"; "#​"))"')

    {
        printf '## Upstream: %s %s\n\n' "$UPSTREAM_PACKAGE" "$latest"
        printf 'immich-rs matches %s. [Release notes](https://github.com/%s/releases/tag/v%s), [all changes](https://github.com/%s/compare/v%s...v%s).\n\n' \
            "$ported" "$UPSTREAM_REPO" "$latest" "$UPSTREAM_REPO" "$ported" "$latest"
        if [[ -n "$commits" ]]; then
            printf 'Commits under `%s` to backport:\n\n%s\n\n' "$UPSTREAM_PATH" "$commits"
        else
            printf 'Only version bumps and dependency chores under `%s`: nothing to backport. (If the CLI moved out of that folder, this list is empty too.)\n\n' "$UPSTREAM_PATH"
        fi
        printf 'Then set `version = "%s"` in Cargo.toml and tag `v%s`.\n\n' "$latest" "$latest"
    } >>"$REPORT"
}

: >"$REPORT"
check_toolchain
check_advisories
check_dependencies
check_upstream

#!/usr/bin/env bash
# Release tags and promotion, behind `just release`, `just upload` and the
# hidden `just _tag`.
#
#   release.sh tag [X.Y.Z]      tag main as vX.Y.Z and push it, replacing an
#                               old vX.Y.Z locally and on origin; the push
#                               starts release.yml
#   release.sh release [X.Y.Z]  tag, then watch that release.yml run
#   release.sh upload [X.Y.Z]   run promote.yml (latest.json -> X.Y.Z) and
#                               watch it
#
# Without a version, tag and release take app/Cargo.toml's version if it has
# no tag yet, else the newest tag's patch + 1; upload takes the newest version
# whose release succeeded. When the version differs from app/Cargo.toml,
# tag commits "Release vX.Y.Z" (Cargo.toml and app/Cargo.lock) to main and
# pushes it first, since release.yml refuses a tag that disagrees with it.
#
# A version that release.yml already uploaded can't be tagged again: the
# guard never overwrites releases/vX.Y.Z/ in R2. Release the next version.
set -euo pipefail

REPO=buwiz-corp/buwiz-oss
cd "$(dirname "$0")/.."

die() {
    echo "release: $*" >&2
    exit 1
}

usage() {
    die "usage: release.sh tag|release|upload [X.Y.Z]"
}

cargo_version() {
    awk -F '"' '
        /^\[package\]$/ { in_package = 1; next }
        in_package && /^\[/ { exit }
        in_package && /^version[[:space:]]*=/ { print $2; exit }
    ' app/Cargo.toml
}

newest_tag() {
    git tag -l 'v*' | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | sort -V | tail -1 || true
}

check_version() {
    [[ $1 =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "version must be MAJOR.MINOR.PATCH, not '$1'"
}

# Database ids of a workflow's runs, newest first.
run_ids() {
    gh run list -R "$REPO" -w "$1" --limit 20 --json databaseId -q '.[].databaseId'
}

# Waits for a run of workflow $1 that isn't in $2 (ids from run_ids before
# starting it), then follows it to the end; fails when the run fails.
watch_new_run() {
    local workflow=$1 before=$2 id=""
    for _ in $(seq 1 30); do
        id=$(run_ids "$workflow" | grep -vxF -f <(printf '%s\n' "$before") | head -1 || true)
        [ -n "$id" ] && break
        sleep 2
    done
    [ -n "$id" ] || die "no new $workflow run showed up; see https://github.com/$REPO/actions"
    echo "watching https://github.com/$REPO/actions/runs/$id"
    gh run watch "$id" -R "$REPO" --exit-status
}

# Makes main the checked-out, clean, up-to-date branch, with origin's tags.
sync_main() {
    [ "$(git branch --show-current)" = main ] || die "check out main first"
    [ -z "$(git status --porcelain --untracked-files=no)" ] || die "commit or stash your changes first"
    git fetch --quiet --force origin main 'refs/tags/*:refs/tags/*'
    git merge --quiet --ff-only origin/main || die "main has commits origin/main lacks; push or drop them first"
}

tag() {
    local version=${1:-}
    version=${version#v}
    sync_main
    if [ -z "$version" ]; then
        local newest
        version=$(cargo_version)
        newest=$(newest_tag)
        if git rev-parse --quiet --verify "refs/tags/v$version" >/dev/null && [ -n "$newest" ]; then
            IFS=. read -r major minor patch <<<"${newest#v}"
            version="$major.$minor.$((patch + 1))"
        fi
    fi
    check_version "$version"
    local tag="v$version"

    local released
    released=$(gh run list -R "$REPO" -w release.yml -b "$tag" -s success --limit 1 --json databaseId -q '.[].databaseId')
    [ -z "$released" ] || die "$tag is already in R2 (run $released); release the next version instead"

    if [ "$(cargo_version)" != "$version" ]; then
        echo "app/Cargo.toml: $(cargo_version) -> $version"
        perl -0pi -e 's/(\[package\][^\[]*?\nversion\s*=\s*")[^"]*"/${1}'"$version"'"/' app/Cargo.toml
        [ "$(cargo_version)" = "$version" ] || die "couldn't set the version in app/Cargo.toml"
        if ! (cd app && RUSTC_WRAPPER='' cargo update --quiet --offline --workspace); then
            git checkout -- app/Cargo.toml app/Cargo.lock
            die "couldn't update app/Cargo.lock (did you run just submodules?)"
        fi
        git commit --quiet -m "Release $tag" app/Cargo.toml app/Cargo.lock
        git push --quiet origin main
    fi

    if git rev-parse --quiet --verify "refs/tags/$tag" >/dev/null; then
        echo "deleting the old local $tag"
        git tag -d "$tag" >/dev/null
    fi
    if git ls-remote --exit-code --tags origin "refs/tags/$tag" >/dev/null; then
        echo "deleting the old $tag on origin"
        git push --quiet origin ":refs/tags/$tag"
    fi
    git tag -a "$tag" -m "Buwiz $version"
    git push --quiet origin "refs/tags/$tag"
    echo "pushed $tag at $(git rev-parse --short HEAD)"
}

release() {
    local before
    before=$(run_ids release.yml)
    tag "$@"
    watch_new_run release.yml "$before"
}

upload() {
    local version=${1:-}
    version=${version#v}
    if [ -z "$version" ]; then
        version=$(gh run list -R "$REPO" -w release.yml -e push -s success --limit 100 --json headBranch -q '.[].headBranch' |
            grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | sort -V | tail -1 || true)
        [ -n "$version" ] || die "no release has succeeded yet; run just release first"
        version=${version#v}
    fi
    check_version "$version"
    local before
    before=$(run_ids promote.yml)
    gh workflow run promote.yml -R "$REPO" -f version="$version"
    echo "promoting $version to latest.json"
    watch_new_run promote.yml "$before"
}

[ $# -ge 1 ] && [ $# -le 2 ] || usage
case $1 in
    tag | release | upload) "$@" ;;
    *) usage ;;
esac

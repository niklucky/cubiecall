#!/bin/sh
# Publish an already verified Windows zip; never rebuild during deployment.
# CI only: release builds need a Windows host (see docs/DEPLOY.md).
set -eu
out=${1:-target/windows}
host=${CUBIE_HOST:?Set CUBIE_HOST to the release account (user@host)}
stage_root=${CUBIE_STAGING_ROOT:-/opt/cubie/mac-staging}
case "$stage_root" in /opt/cubie|/opt/cubie/mac-staging) ;; *) echo 'Unexpected staging root' >&2; exit 1 ;; esac
build=$(sed -n 's/.*(\([0-9][0-9]*\))$/\1/p' "$out/version.txt")
case "$build" in ''|*[!0-9]*) echo 'Invalid build number' >&2; exit 1 ;; esac
instructions=$(mktemp -d)
stage=
cleanup() {
    rm -rf "$instructions"
    # shellcheck disable=SC2029
    [ -z "$stage" ] || ssh -n "$host" "rm -rf -- $stage" || true
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM
python3 deploy/render-site.py --downloads-only "$instructions"
# shellcheck disable=SC2029
remote_stage=$(ssh -n "$host" "mktemp -d $stage_root/.windows-release.XXXXXXXX")
case "$remote_stage" in "$stage_root"/.windows-release.*) stage=$remote_stage ;; *) echo 'Unexpected staging path' >&2; exit 1 ;; esac
scp -q "$out/Cubie-windows-x64.zip" "$out/version.txt" "$out/SHA256SUMS" \
    "$instructions/download/index.html" "$host:$stage/"
# These paths have a fixed prefix and the build is numeric.
# shellcheck disable=SC2029
ssh "$host" "sh -s -- $stage" <<'REMOTE'
set -eu
cd "$1"
sha256sum -c SHA256SUMS
REMOTE
# shellcheck disable=SC2029
ssh "$host" "sh -s -- /opt/cubie/download $stage $build" < deploy/windows/publish.sh
# The page follows the zip, as the Mac's follows its archives. It shows
# Windows only once the zip answers, so whichever release publishes it
# first, nobody gets a link that isn't there yet.
# shellcheck disable=SC2029
ssh -n "$host" "set -eu; install -m 644 /opt/cubie/download/windows/releases/$build/index.html /opt/cubie/download/.index.html.w$build; mv -f /opt/cubie/download/.index.html.w$build /opt/cubie/download/index.html"

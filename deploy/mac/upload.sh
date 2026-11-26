#!/bin/sh
# Publish already verified artifacts; never rebuild during deployment.
set -eu
out=${1:-target/mac}
host=${CUBIE_HOST:-${QUIPCALL_HOST:?set CUBIE_HOST to the release SSH address}}
: "${CUBIE_MAC_TEAM_ID:?set the Apple developer team ID}"
instructions=$(mktemp -d)
stage=
cleanup() {
    rm -rf "$instructions"
    [ -z "$stage" ] || ssh -n "$host" "rm -rf -- $stage" || true
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM
python3 deploy/render-site.py --downloads-only "$instructions"
stage_root=${CUBIE_STAGING_ROOT:-/opt/cubie}
case "$stage_root" in /opt/cubie|/opt/cubie/mac-staging) ;; *) echo 'Unexpected staging root' >&2; exit 1 ;; esac
build=$(sed -n 's/.*(\([0-9][0-9]*\))$/\1/p' "$out/version.txt")
case "$build" in ''|*[!0-9]*) echo 'Invalid build number' >&2; exit 1 ;; esac
# shellcheck disable=SC2029
remote_stage=$(ssh -n "$host" "mktemp -d $stage_root/.mac-release.XXXXXXXX")
case "$remote_stage" in "$stage_root"/.mac-release.*) stage=$remote_stage ;; *) echo 'Unexpected staging path' >&2; exit 1 ;; esac
scp -q "$out"/Cubie-*.zip "$out"/appcast-*.xml "$out/version.txt" "$out/SHA256SUMS" \
    "$instructions/download/index.html" "$instructions/download/install.sh" "$host:$stage/"
# These paths have a fixed prefix and the build is numeric.
# shellcheck disable=SC2029
ssh "$host" "sh -s -- $stage" <<'REMOTE'
set -eu
cd "$1"
sha256sum -c SHA256SUMS
REMOTE
# shellcheck disable=SC2029
ssh "$host" "sh -s -- /opt/cubie/download $stage $build" < deploy/mac/publish.sh
# Static instructions follow the archive/feed switch, using atomic replacement.
# shellcheck disable=SC2029
ssh -n "$host" "set -eu; for file in index.html install.sh; do install -m 644 /opt/cubie/download/releases/$build/\$file /opt/cubie/download/.\$file.$build; mv -f /opt/cubie/download/.\$file.$build /opt/cubie/download/\$file; done"

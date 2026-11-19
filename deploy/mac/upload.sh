#!/bin/sh
# Publish already verified artifacts; never rebuild during deployment.
set -eu
out=${1:-target/mac}
host=${CUBIE_HOST:-${QUIPCALL_HOST:-root@testron.dev}}
stage_root=${CUBIE_STAGING_ROOT:-/opt/cubie}
case "$stage_root" in /opt/cubie|/opt/cubie/mac-staging) ;; *) echo 'Unexpected staging root' >&2; exit 1 ;; esac
build=$(sed -n 's/.*(\([0-9][0-9]*\))$/\1/p' "$out/version.txt")
case "$build" in ''|*[!0-9]*) echo 'Invalid build number' >&2; exit 1 ;; esac
# shellcheck disable=SC2029
stage=$(ssh -n "$host" "mktemp -d $stage_root/.mac-release.XXXXXXXX")
case "$stage" in "$stage_root"/.mac-release.*) ;; *) echo 'Unexpected staging path' >&2; exit 1 ;; esac
trap 'ssh -n "$host" "rm -rf -- $stage" || true' EXIT
trap 'exit 1' HUP INT TERM
scp -q "$out"/Cubie-*.zip "$out"/appcast-*.xml "$out/version.txt" "$out/SHA256SUMS" \
    deploy/download/index.html deploy/download/install.sh "$host:$stage/"
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

#!/bin/sh
# The two parts of Cubie that change with the code: the API server and the
# web client. Runs on the VPS as root.
#
# deploy/install.sh uses the steps one at a time (`make deploy`). `receive`
# is all of them for a bundle on standard input, and the only thing the
# GitHub Actions deploy key can run (deploy/ci-access.sh): it replaces the
# server and the web client, and can't touch nginx, systemd, LiveKit, the
# secrets or anything else on the box.
set -eu
umask 022
ROOT=/opt/cubie
STATE=/var/lib/cubie
PREVIOUS=$ROOT/previous
LIMIT=268435456 # bytes of bundle, unpacked: 256 MiB
WAIT=300        # seconds for a bundle to arrive

die() {
    echo "$*" >&2
    exit 1
}

installed() {
    sha256sum "$ROOT/bin/cubie-server" 2>/dev/null | cut -d' ' -f1
}

# The API answers within half a minute, long enough to migrate the database.
healthy() {
    i=0
    until curl -fsS http://127.0.0.1:4601/api/health >/dev/null 2>&1; do
        i=$((i + 1))
        [ $i -ge 60 ] && return 1
        sleep 0.5
    done
}

# What a rollback needs. A new server may migrate the database, and the one
# it replaces refuses a database newer than itself, so going back takes the
# old server, the old web client and the database as they were together.
# Taken only from a deployment that finished (or, the first time this runs,
# one that answers): a retry after a failed attempt must not replace it with
# that attempt's own half-upgraded state. `restart` adds the database.
keep() {
    rm -rf "$PREVIOUS.new"
    [ -n "$(installed)" ] && [ -f "$STATE/cubie.db" ] || return 0
    if [ -f "$ROOT/.deployed" ]; then
        [ "$(cat "$ROOT/.deployed")" = "$(installed)" ] || return 0
    else
        curl -fsS http://127.0.0.1:4601/api/health >/dev/null 2>&1 || return 0
    fi
    mkdir -m 700 "$PREVIOUS.new"
    cp -p "$ROOT/bin/cubie-server" "$PREVIOUS.new/"
    [ ! -d "$ROOT/web" ] || cp -R "$ROOT/web" "$PREVIOUS.new/web"
}

# Put the server and web client from directory $1 in place. The web client
# changes as a whole directory, at once.
swap() {
    install -m 755 "$1/cubie-server" "$ROOT/bin/cubie-server.new"
    mv "$ROOT/bin/cubie-server.new" "$ROOT/bin/cubie-server"
    rm -rf "$ROOT/web.new" "$ROOT/web.old"
    cp -R "$1/web" "$ROOT/web.new"
    chmod -R u=rwX,go=rX "$ROOT/web.new"
    [ ! -d "$ROOT/web" ] || mv "$ROOT/web" "$ROOT/web.old"
    mv "$ROOT/web.new" "$ROOT/web"
    rm -rf "$ROOT/web.old"
}

# Start the server that `swap` put in place. If `keep` began a rollback copy,
# finish it first with the database as the old server left it: stopped, so
# the copy (with its write-ahead log) is whole.
restart() {
    if [ -d "$PREVIOUS.new" ]; then
        systemctl stop cubie-server
        for suffix in '' -wal -shm; do
            [ ! -f "$STATE/cubie.db$suffix" ] || cp -p "$STATE/cubie.db$suffix" "$PREVIOUS.new/"
        done
        rm -rf "$PREVIOUS"
        mv "$PREVIOUS.new" "$PREVIOUS"
        echo "kept the previous server, web client and database in $PREVIOUS"
    fi
    systemctl restart cubie-server
}

# This server is now the one a later deploy may keep for a rollback.
mark() {
    installed > "$ROOT/.deployed.new"
    mv "$ROOT/.deployed.new" "$ROOT/.deployed"
}

# Everything `keep` and `restart` set aside, back in place: the documented
# rollback (docs/DEPLOY.md).
put_back() {
    systemctl stop cubie-server
    rm -f "$STATE/cubie.db-wal" "$STATE/cubie.db-shm"
    cp -p "$PREVIOUS"/cubie.db* "$STATE/"
    swap "$PREVIOUS"
    systemctl start cubie-server
}

# A bundle from GitHub Actions on standard input: a gzipped tar of
# `cubie-server`, `web/` and `revision` (the commit it was built from).
# Nothing in it runs as root: the server runs as the `cubie` user, and the
# rest is copied as plain files.
receive() {
    [ ! -e "$ROOT/.cutover-pending" ] && [ ! -e "$ROOT/.legacy-services" ] ||
        die 'A deployment from the release Mac is unfinished. Finish it with "make deploy".'
    [ -x "$ROOT/bin/cubie-server" ] && [ -d "$ROOT/web" ] && [ -f /etc/systemd/system/cubie-server.service ] ||
        die 'Cubie is not installed here yet. Run "make deploy" first.'

    # One deployment at a time, `make deploy` included. A connection that
    # sends nothing holds this up for five minutes, not for good.
    exec 9>/run/lock/cubie-deploy.lock
    flock -w 600 9 || die 'Another deployment is still running.'

    # One fixed place: what an attempt that was killed left is gone with
    # the next one.
    umask 077
    stage=$ROOT/incoming
    rm -rf "$stage"
    mkdir "$stage"
    trap 'rm -rf "$stage"' EXIT
    trap 'exit 1' HUP INT TERM
    timeout "$WAIT" gzip -dc | head -c "$LIMIT" | tar -xf - -C "$stage" --no-same-owner --no-same-permissions ||
        die 'Not a bundle (or larger than 256 MiB, or five minutes in coming).'
    [ -z "$(find "$stage" ! -type f ! -type d | head -n 1)" ] ||
        die 'A bundle holds plain files and directories only.'
    [ -z "$(find "$stage" -mindepth 1 -maxdepth 1 ! -name cubie-server ! -name revision ! -name web | head -n 1)" ] &&
        [ -f "$stage/cubie-server" ] && [ -f "$stage/revision" ] && [ -f "$stage/web/index.html" ] ||
        die 'A bundle is cubie-server, web/ and revision.'
    revision=$(cat "$stage/revision")
    case "$revision" in *[!0-9a-f]*) die 'revision is not a commit.' ;; esac
    [ ${#revision} -eq 40 ] || die 'revision is not a commit.'
    # Whatever modes the archive asked for.
    find "$stage" -type d -exec chmod 755 {} +
    find "$stage" -type f -exec chmod 644 {} +
    umask 022

    # The same build again (a job run twice) changes nothing, and so keeps
    # the rollback copy of the version before it.
    if [ "$(cat "$ROOT/.deployed" 2>/dev/null)" = "$(installed)" ] &&
        cmp -s "$stage/cubie-server" "$ROOT/bin/cubie-server" &&
        diff -rq "$stage/web" "$ROOT/web" >/dev/null; then
        echo "$revision is already deployed"
        return 0
    fi

    keep
    kept=
    [ ! -d "$PREVIOUS.new" ] || kept=1
    swap "$stage"
    restart
    if ! healthy; then
        journalctl -u cubie-server -n 30 --no-pager || true
        [ -n "$kept" ] || die "$revision doesn't answer, and there was nothing to put back."
        put_back
        healthy || die "$revision doesn't answer, and neither does the server it replaced."
        die "$revision doesn't answer. The server, web client and database it replaced are back."
    fi
    mark
    echo "$revision" > "$ROOT/.revision"
    echo "deployed $revision"
}

case "${1:-}" in
    keep | restart | mark | receive) [ $# -eq 1 ] || die "usage: $0 $1" ;;
    swap) [ $# -eq 2 ] || die "usage: $0 swap <directory>" ;;
    *) die "usage: $0 keep | swap <directory> | restart | mark | receive" ;;
esac
"$@"

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

# The database's directory belongs to the server, and the server is whatever
# was last deployed. Root never opens a file there: a link left in it would
# point root anywhere on the box. Reading and writing it is done as `cubie`.
as_cubie() {
    runuser -u cubie -- "$@"
}

# One deployment or rollback at a time, `make deploy` included (deploy.sh
# takes the same lock around install.sh).
lock() {
    exec 9>/run/lock/cubie-deploy.lock
    flock -w 600 9 || die 'Another deployment is still running.'
}

# The API answers within half a minute, long enough to migrate the database.
healthy() {
    i=0
    until curl -fsS -m 2 http://127.0.0.1:4601/api/health >/dev/null 2>&1; do
        i=$((i + 1))
        [ $i -ge 30 ] && return 1
        sleep 1
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
        curl -fsS -m 5 http://127.0.0.1:4601/api/health >/dev/null 2>&1 || return 0
    fi
    mkdir -m 700 "$PREVIOUS.new"
    cp -p "$ROOT/bin/cubie-server" "$PREVIOUS.new/"
    [ ! -f "$ROOT/.revision" ] || cp "$ROOT/.revision" "$PREVIOUS.new/revision"
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
            [ ! -f "$STATE/cubie.db$suffix" ] ||
                as_cubie cat "$STATE/cubie.db$suffix" > "$PREVIOUS.new/cubie.db$suffix"
        done
        rm -rf "$PREVIOUS"
        mv "$PREVIOUS.new" "$PREVIOUS"
        echo "kept the previous server, web client and database in $PREVIOUS"
    fi
    # A server that won't start is the health check's to report, so that
    # whoever waits for it can still put the previous one back.
    systemctl restart cubie-server || echo "cubie-server did not start" >&2
}

# This server is now the one a later deploy may keep for a rollback. Which
# commit it is, only `receive` and `back` know: they say so after this.
mark() {
    installed > "$ROOT/.deployed.new"
    mv "$ROOT/.deployed.new" "$ROOT/.deployed"
    rm -f "$ROOT/.revision"
}

# Everything `keep` and `restart` set aside, back in place (`back`, the
# rollback in docs/DEPLOY.md). It loses what was written since.
put_back() {
    [ -f "$PREVIOUS/cubie-server" ] && [ -d "$PREVIOUS/web" ] && [ -f "$PREVIOUS/cubie.db" ] ||
        die "There is no complete copy in $PREVIOUS."
    systemctl stop cubie-server
    for suffix in '' -wal -shm; do
        as_cubie rm -f "$STATE/cubie.db$suffix"
        # shellcheck disable=SC2016
        [ ! -f "$PREVIOUS/cubie.db$suffix" ] ||
            as_cubie sh -c 'umask 077 && cat > "$1"' sh "$STATE/cubie.db$suffix" < "$PREVIOUS/cubie.db$suffix"
    done
    swap "$PREVIOUS"
    systemctl start cubie-server
}

back() {
    lock
    put_back
    healthy || die "The previous server is back in place, and doesn't answer."
    # It is what runs now: the next deploy keeps a copy of it, not of the
    # one it replaced.
    mark
    [ ! -f "$PREVIOUS/revision" ] || cp "$PREVIOUS/revision" "$ROOT/.revision"
    echo "the previous server, web client and database are back"
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

    # A connection that sends nothing holds the lock for five minutes, not
    # for good.
    lock

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
    keep | restart | mark | receive | back) [ $# -eq 1 ] || die "usage: $0 $1" ;;
    swap) [ $# -eq 2 ] || die "usage: $0 swap <directory>" ;;
    *) die "usage: $0 keep | swap <directory> | restart | mark | receive | back" ;;
esac
"$@"

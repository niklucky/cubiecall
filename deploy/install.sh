#!/bin/sh
# Runs on the VPS as root, from the bundle deploy.sh uploads. Safe to re-run.
#
# Layout: /opt/cubie/{bin,web,download}, secrets.env (API keys, made here
# once), livekit.yaml, previous/ (what the last deploy replaced, for a
# rollback: docs/DEPLOY.md), ci/ (the key GitHub Actions deploys with);
# systemd units cubie-livekit and cubie-server; nginx site cubiecall.com;
# the cubie and cubie-deploy accounts and the second one's sudo rule
# (deploy/ci-access.sh). Nothing else on the box is touched.
set -eu
umask 022
STAGE=$(cd "$(dirname "$0")/.." && pwd)
ROOT=/opt/cubie
DOMAIN=cubiecall.com
OLD_ROOT=/opt/quipcall
OLD_SITE=/etc/nginx/sites-enabled/quipcall.com.conf
STATE=/var/lib/cubie
OLD_STATE=/var/lib/quipcall
SITE=/etc/nginx/sites-available/$DOMAIN.conf
ENABLED_SITE=/etc/nginx/sites-enabled/$DOMAIN.conf
LIVEKIT_VERSION=1.13.11

# Install $1 as $2 with mode $3 unless identical.
put() {
	cmp -s "$1" "$2" && return 0
	install -m "$3" "$1" "$2" || exit 1
	echo "updated $2"
}

# Keep the previous site recoverable until the shared nginx accepts this one.
# During bootstrap, the existing Quipcall HTTPS site must remain enabled.
activate_site() (
    source=$1
    old_link=
    if [ "$2" = final ] && [ -L "$OLD_SITE" ]; then
        old_link=$(readlink "$OLD_SITE")
    fi
    backup="$STAGE/previous-site"
    rm -f "$backup"
    if [ -f "$SITE" ]; then cp "$SITE" "$backup"; fi
    enabled_link=
    if [ -L "$ENABLED_SITE" ]; then enabled_link=$(readlink "$ENABLED_SITE"); fi
    install -m 644 "$source" "$SITE"
    ln -sf "$SITE" "$ENABLED_SITE"
    if [ -n "$old_link" ]; then rm "$OLD_SITE"; fi
    if nginx -t && systemctl reload nginx; then
        echo "updated nginx site $DOMAIN"
    else
        if [ -f "$backup" ]; then mv "$backup" "$SITE"; else rm -f "$SITE"; fi
        if [ -n "$enabled_link" ]; then ln -sf "$enabled_link" "$ENABLED_SITE"; else rm -f "$ENABLED_SITE"; fi
        if [ -n "$old_link" ]; then ln -s "$old_link" "$OLD_SITE"; fi
        exit 1
    fi
)

# Retry a check for up to 5 seconds.
eventually() {
	i=0
	until "$@" >/dev/null 2>&1; do
		i=$((i + 1))
		[ $i -ge 10 ] && return 1
		sleep 0.5
	done
}

# Recover a definitely unpublished attempt before any network or setup step
# can fail again. Once the old vhost has been removed in the publishing phase,
# a client may have written to Cubie and retries must keep its database.
services="$ROOT/.legacy-services"
phase=$(cat "$STATE/.from-quipcall" 2>/dev/null || true)
restore_services() {
    systemctl disable --now cubie-server cubie-livekit || true
    if [ -f "$STATE/.from-quipcall" ]; then rm -rf "$STATE"; fi
    while read -r action unit; do systemctl "$action" "$unit"; done < "$services"
    rm -f "$services" "$ROOT/.cutover-pending"
    echo "Restored the previous services." >&2
}
if [ -f "$services" ] && { [ "$phase" = unpublished ] || [ ! -e "$STATE" ] ||
    { [ "$phase" = publishing ] && [ -L "$OLD_SITE" ]; }; }; then
    restore_services
    phase=
fi

id cubie >/dev/null 2>&1 ||
	useradd --system --home-dir $ROOT --no-create-home --shell /usr/sbin/nologin cubie
mkdir -p "$ROOT/bin" "$ROOT/download"
# Uploads use a private staging directory. The public web and download roots
# need to be traversable both after a fresh install and after prepublishing zips.
chmod 755 "$ROOT" "$ROOT/download"

# The old site keeps serving its HTTPS app until the new names have a
# certificate. Both ACME sites must see the same challenge files meanwhile.
if [ -d /var/www/quipcall-acme ] && [ ! -e /var/www/cubie-acme ]; then
    ln -s /var/www/quipcall-acme /var/www/cubie-acme
fi
mkdir -p /var/www/cubie-acme

# The legacy directory is the rollback copy. A marker in the new directory
# distinguishes a previous copy made by this installer from an unrelated DB.
upgrade=
if [ -d "$OLD_STATE" ]; then
    if [ -e "$STATE" ]; then
        case "$phase" in
            unpublished|publishing|live) ;;
            *) echo "Both $OLD_STATE and $STATE exist. Reconcile them before deploying." >&2; exit 1 ;;
        esac
    fi
    if [ ! -e "$STATE" ] || [ "$phase" = unpublished ] ||
       { [ "$phase" = publishing ] && [ -L "$OLD_SITE" ]; }; then
        upgrade=1
        for suffix in '' -wal -shm -journal; do
            if [ -e "$OLD_STATE/cubie.db$suffix" ]; then
                echo "Unexpected Cubie SQLite file in $OLD_STATE. Reconcile it before deploying." >&2
                exit 1
            fi
        done
    fi
fi
if [ -f "$OLD_ROOT/secrets.env" ] && [ ! -f "$ROOT/secrets.env" ]; then
    install -m 600 "$OLD_ROOT/secrets.env" "$ROOT/secrets.env"
fi
for archive in "$OLD_ROOT"/download/Quipcall-*.zip; do
    [ -f "$archive" ] || continue
    [ -f "$ROOT/download/${archive##*/}" ] || install -m 644 "$archive" "$ROOT/download/"
done

# A first install needs a temporary HTTP site. On upgrade the old active
# vhost already serves challenges for its own two names.
if [ ! -f "/etc/letsencrypt/live/$DOMAIN/fullchain.pem" ]; then
    bootstrap="$STAGE/deploy/nginx/$DOMAIN.acme.conf"
    if [ -e "$OLD_SITE" ]; then
        sed 's/ quipcall.com www.quipcall.com;/;/' "$bootstrap" > "$STAGE/bootstrap.conf"
        bootstrap="$STAGE/bootstrap.conf"
    fi
    activate_site "$bootstrap" bootstrap
fi

# All three public domains must validate. Add their www aliases as DNS
# becomes available, and retain any already covered by the current certificate.
set -- -d "$DOMAIN" -d cubycall.com -d quipcall.com
cert="/etc/letsencrypt/live/$DOMAIN/fullchain.pem"
for host in "www.$DOMAIN" www.cubycall.com www.quipcall.com; do
    if getent ahosts "$host" >/dev/null ||
        { [ -s "$cert" ] && openssl x509 -in "$cert" -noout -ext subjectAltName | tr ', ' '\n' | grep -Fxq "DNS:$host"; }; then
        set -- "$@" -d "$host"
    else
        echo "Deferring TLS for $host until its DNS record exists."
    fi
done
certbot certonly --cert-name "$DOMAIN" --webroot -w /var/www/cubie-acme "$@" \
    --key-type ecdsa --keep-until-expiring --expand --non-interactive --agree-tos

# LiveKit restarts drop every call in progress, so only when something changed.
if ! $ROOT/bin/livekit-server --version 2>/dev/null | grep -qx "livekit-server version $LIVEKIT_VERSION"; then
	tmp=$(mktemp -d)
	curl -fsSL "https://github.com/livekit/livekit/releases/download/v$LIVEKIT_VERSION/livekit_${LIVEKIT_VERSION}_linux_amd64.tar.gz" |
		tar -xz -C "$tmp" livekit-server
	install -m 755 "$tmp/livekit-server" $ROOT/bin/livekit-server
	rm -rf "$tmp"
	echo "installed LiveKit $LIVEKIT_VERSION"
fi

# Merge supplied auth settings and fill missing LiveKit keys without rotating
# an existing pair. This also supports an auth-only, pre-created secrets.env.
if [ -f "$STAGE/deploy/auth.env" ]; then
    python3 "$STAGE/deploy/configure-secrets.py" "$ROOT/secrets.env" "$STAGE/deploy/auth.env"
else
    python3 "$STAGE/deploy/configure-secrets.py" "$ROOT/secrets.env"
fi


# Keep what this deploy replaces, for a rollback (deploy/app.sh). The first
# cutover has its own way back: the legacy services and their database.
app() { sh "$STAGE/deploy/app.sh" "$@"; }
[ -n "$upgrade" ] || app keep

put "$STAGE/deploy/livekit.yaml" $ROOT/livekit.yaml 644
put "$STAGE/deploy/systemd/cubie-livekit.service" /etc/systemd/system/cubie-livekit.service 644
put "$STAGE/deploy/systemd/cubie-server.service" /etc/systemd/system/cubie-server.service 644
app swap "$STAGE"
install -m 644 "$STAGE/deploy/download/index.html" "$STAGE/deploy/download/install.sh" $ROOT/download/

systemctl daemon-reload
if [ -n "$upgrade" ]; then
    # Keep the legacy database and services recoverable until nginx publishes
    # the healthy candidate. Its API uses a separate loopback port, so the
    # old site cannot send writes to a candidate we might discard.
    touch "$ROOT/.cutover-pending"
    if [ ! -f "$services" ]; then
        : > "$services.new"
        for unit in quipcall-livekit quipcall-server; do
            if systemctl is-active --quiet "$unit"; then echo "start $unit" >> "$services.new"; fi
            if systemctl is-enabled --quiet "$unit"; then echo "enable $unit" >> "$services.new"; fi
        done
        mv "$services.new" "$services"
    fi
    candidate=$(mktemp -d /var/lib/.cubie-upgrade.XXXXXX)
    restore_legacy() {
        status=$?
        trap - EXIT HUP INT TERM
        if [ "$status" -ne 0 ]; then
            # Once a reload may have published the new DB, retry forward.
            if [ "$phase" != publishing ] || [ -L "$OLD_SITE" ]; then
                restore_services
            fi
        fi
        rm -rf "$candidate"
        exit "$status"
    }
    trap restore_legacy EXIT
    trap 'exit 1' HUP INT TERM
    while read -r action unit; do
        case "$action" in
            start) systemctl stop "$unit" ;;
            enable) systemctl disable "$unit" ;;
        esac
    done < "$services"

    if [ -e "$STATE" ]; then
        # An interrupted attempt left an unpublished copy. The old vhost
        # is still enabled and all user writes went to the original DB.
        systemctl stop cubie-server cubie-livekit
        rm -rf "$STATE"
    fi
    cp -a "$OLD_STATE/." "$candidate/"
    for suffix in -wal -shm -journal ''; do
        [ ! -f "$candidate/quipcall.db$suffix" ] || mv "$candidate/quipcall.db$suffix" "$candidate/cubie.db$suffix"
    done
    echo unpublished > "$candidate/.from-quipcall"
    chown -R cubie:cubie "$candidate"
    mv "$candidate" "$STATE"
fi

# Record the revision only after the service is healthy. A retry after a
# partial install must restart a process still running the previous revision.
livekit_revision=$({
    sha256sum "$ROOT/bin/livekit-server" "$ROOT/livekit.yaml" /etc/systemd/system/cubie-livekit.service
    sed -n '/^LIVEKIT_KEYS=/p' "$ROOT/secrets.env"
} | sha256sum)
systemctl enable --quiet cubie-livekit cubie-server
if [ "$(cat "$ROOT/.running-livekit" 2>/dev/null || true)" != "$livekit_revision" ]; then
    systemctl restart cubie-livekit
else
    systemctl start cubie-livekit
fi
app restart

for unit in cubie-livekit cubie-server; do
	if ! eventually systemctl is-active --quiet $unit; then
		journalctl -u $unit -n 30 --no-pager
		exit 1
	fi
done
eventually curl -fsS http://127.0.0.1:7880/ || {
	echo "LiveKit isn't answering" >&2
	exit 1
}
printf '%s\n' "$livekit_revision" > "$ROOT/.running-livekit.new"
mv "$ROOT/.running-livekit.new" "$ROOT/.running-livekit"
eventually curl -fsS http://127.0.0.1:4601/api/health || {
    echo "Cubie API failed its readiness check" >&2
    exit 1
}
if [ -n "$upgrade" ]; then
    phase=publishing
    echo publishing > "$STATE/.from-quipcall.new"
    mv "$STATE/.from-quipcall.new" "$STATE/.from-quipcall"
fi
activate_site "$STAGE/deploy/nginx/$DOMAIN.conf" final
# Requests can now write to Cubie. Never restore a snapshot after this point.
trap - EXIT HUP INT TERM
if [ "$phase" = publishing ]; then
    echo live > "$STATE/.from-quipcall.new"
    mv "$STATE/.from-quipcall.new" "$STATE/.from-quipcall"
fi
rm -f "$ROOT/.legacy-services" "$ROOT/.cutover-pending"
eventually curl -fsS --resolve $DOMAIN:443:127.0.0.1 https://$DOMAIN/api/health || {
	echo "https://$DOMAIN/api/health failed" >&2
	journalctl -u cubie-server -n 30 --no-pager
	exit 1
}
app mark
# GitHub Actions deploys the server and web client from here on.
sh "$STAGE/deploy/ci-access.sh"
echo "deployed: https://$DOMAIN"

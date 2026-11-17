#!/bin/sh
# Runs on the VPS as root, from the bundle deploy.sh uploads. Safe to re-run.
#
# Layout: /opt/cubie/{bin,web,download}, secrets.env (API keys, made here
# once), livekit.yaml; systemd units cubie-livekit and cubie-server;
# nginx site cubiecall.com. Nothing else on the box is touched.
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

# Install $1 as $2 with mode $3 unless identical. Fails if nothing changed.
put() {
	cmp -s "$1" "$2" && return 1
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
    if cmp -s "$source" "$SITE" && [ -L "$ENABLED_SITE" ] && [ -z "$old_link" ]; then
        return
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

id cubie >/dev/null 2>&1 ||
	useradd --system --home-dir $ROOT --no-create-home --shell /usr/sbin/nologin cubie
mkdir -p "$ROOT/bin" "$ROOT/download"
# Uploads use a private staging directory. The public web and download roots
# need to be traversable both after a fresh install and after prepublishing zips.
chmod 755 "$ROOT" "$ROOT/download"

# The old site keeps serving its HTTPS app until all six names have a
# certificate. Both ACME sites must see the same challenge files meanwhile.
if [ -d /var/www/quipcall-acme ] && [ ! -e /var/www/cubie-acme ]; then
    ln -s /var/www/quipcall-acme /var/www/cubie-acme
fi
mkdir -p /var/www/cubie-acme

if [ -d "$OLD_STATE" ] && [ -e "$STATE" ]; then
    echo "Both $OLD_STATE and $STATE exist. Reconcile them before deploying." >&2
    exit 1
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

certbot certonly --cert-name "$DOMAIN" --webroot -w /var/www/cubie-acme \
    -d "$DOMAIN" -d "www.$DOMAIN" -d cubycall.com -d www.cubycall.com -d quipcall.com -d www.quipcall.com \
    --key-type ecdsa --keep-until-expiring --expand --non-interactive --agree-tos

# LiveKit restarts drop every call in progress, so only when something changed.
restart_livekit=
if ! $ROOT/bin/livekit-server --version 2>/dev/null | grep -qx "livekit-server version $LIVEKIT_VERSION"; then
	tmp=$(mktemp -d)
	curl -fsSL "https://github.com/livekit/livekit/releases/download/v$LIVEKIT_VERSION/livekit_${LIVEKIT_VERSION}_linux_amd64.tar.gz" |
		tar -xz -C "$tmp" livekit-server
	install -m 755 "$tmp/livekit-server" $ROOT/bin/livekit-server
	rm -rf "$tmp"
	echo "installed LiveKit $LIVEKIT_VERSION"
	restart_livekit=1
fi

# Merge supplied auth settings and fill missing LiveKit keys without rotating
# an existing pair. This also supports an auth-only, pre-created secrets.env.
if [ -f "$STAGE/deploy/auth.env" ]; then
    keys_changed=$(python3 "$STAGE/deploy/configure-secrets.py" "$ROOT/secrets.env" "$STAGE/deploy/auth.env")
else
    keys_changed=$(python3 "$STAGE/deploy/configure-secrets.py" "$ROOT/secrets.env")
fi
[ -z "$keys_changed" ] || restart_livekit=1

put "$STAGE/deploy/livekit.yaml" $ROOT/livekit.yaml 644 && restart_livekit=1
put "$STAGE/deploy/systemd/cubie-livekit.service" /etc/systemd/system/cubie-livekit.service 644 &&
	restart_livekit=1
put "$STAGE/deploy/systemd/cubie-server.service" /etc/systemd/system/cubie-server.service 644 || true
install -m 755 "$STAGE/cubie-server" $ROOT/bin/cubie-server.new
mv $ROOT/bin/cubie-server.new $ROOT/bin/cubie-server

# Web client: swap the whole directory at once.
rm -rf $ROOT/web.new $ROOT/web.old
cp -R "$STAGE/web" $ROOT/web.new
chmod -R u=rwX,go=rX $ROOT/web.new
[ -d $ROOT/web ] && mv $ROOT/web $ROOT/web.old
mv $ROOT/web.new $ROOT/web
rm -rf $ROOT/web.old
install -m 644 "$STAGE/deploy/download/index.html" "$STAGE/deploy/download/install.sh" $ROOT/download/

activate_site "$STAGE/deploy/nginx/$DOMAIN.conf" final

# Cut over after the new certificate and nginx config pass validation.
for unit in quipcall-server quipcall-livekit; do
    if systemctl is-active --quiet "$unit" || systemctl is-enabled --quiet "$unit"; then
        systemctl disable --now "$unit"
    fi
done
if [ -d "$OLD_STATE" ]; then
    mv "$OLD_STATE" "$STATE"
fi
# Retain uncheckpointed WAL and rollback journals as well as the database.
# Move the database last so an interrupted rename can resume with writers off.
if [ -f "$STATE/quipcall.db" ]; then
    for suffix in -wal -shm -journal ''; do
        if [ -e "$STATE/quipcall.db$suffix" ] && [ -e "$STATE/cubie.db$suffix" ]; then
            echo "Both legacy and Cubie SQLite files exist for suffix '$suffix'." >&2
            exit 1
        fi
    done
    chown -R cubie:cubie "$STATE"
    for suffix in -wal -shm -journal ''; do
        [ ! -f "$STATE/quipcall.db$suffix" ] || mv "$STATE/quipcall.db$suffix" "$STATE/cubie.db$suffix"
    done
fi

systemctl daemon-reload
systemctl enable --quiet cubie-livekit cubie-server
if [ -n "$restart_livekit" ]; then
	systemctl restart cubie-livekit
	echo "restarted LiveKit"
else
	systemctl start cubie-livekit
fi
systemctl restart cubie-server

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
eventually curl -fsS --resolve $DOMAIN:443:127.0.0.1 https://$DOMAIN/api/health || {
	echo "https://$DOMAIN/api/health failed" >&2
	journalctl -u cubie-server -n 30 --no-pager
	exit 1
}
echo "deployed: https://$DOMAIN"

#!/bin/sh
# Exercise the first production cutover using a disposable Linux filesystem.
set -eu
cd "$(dirname "$0")/.."
docker run --rm -i --network none -v "$PWD/deploy:/source:ro" python:3.11-slim-bookworm sh -se <<'CONTAINER'
mkdir -p /bundle/web /opt/cubie/bin /opt/quipcall /var/lib/quipcall /etc/nginx/sites-enabled /etc/nginx/sites-available /etc/systemd/system /etc/letsencrypt/live/cubiecall.com /mock/bin /mock/units
cp -R /source /bundle/deploy
touch /bundle/cubie-server /etc/letsencrypt/live/cubiecall.com/fullchain.pem
echo legacy > /etc/nginx/sites-available/quipcall.com.conf
ln -s ../sites-available/quipcall.com.conf /etc/nginx/sites-enabled/quipcall.com.conf
for name in quipcall-livekit quipcall-server; do touch "/mock/units/$name.active" "/mock/units/$name.enabled"; done
cat >/opt/quipcall/secrets.env <<'ENV'
RESEND_API_KEY=test
QUIPCALL_MAIL_FROM="Quipcall <test@example.test>"
QUIPCALL_BOOTSTRAP_EMAIL=admin@example.test
LIVEKIT_KEYS="existing-key: existing-secret"
ENV
cat >/opt/cubie/bin/livekit-server <<'SH'
#!/bin/sh
echo 'livekit-server version 1.13.11'
SH
cat >/mock/bin/systemctl <<'SH'
#!/bin/sh
set -eu
action=$1
shift
for unit in "$@"; do
    case "$unit" in -*) continue ;; esac
    case "$action" in
        is-active) test -e "/mock/units/$unit.active" ;;
        is-enabled) test -e "/mock/units/$unit.enabled" ;;
        enable) touch "/mock/units/$unit.enabled" ;;
        disable) rm -f "/mock/units/$unit.enabled"; if [ "${1:-}" = --now ]; then rm -f "/mock/units/$unit.active"; fi ;;
        start|restart) touch "/mock/units/$unit.active"; echo "$action $unit" >>/mock/service-log ;;
        stop) rm -f "/mock/units/$unit.active" ;;
        reload) test ! -e /mock/fail-nginx ;;
        *) exit 1 ;;
    esac
done
if [ "$action" = daemon-reload ]; then test ! -e /mock/fail-daemon-reload; fi
SH
cat >/mock/bin/curl <<'SH'
#!/bin/sh
case "$*" in *4601/api/health*) test ! -e /mock/fail-api ;; *) exit 0 ;; esac
SH
for name in nginx certbot sleep journalctl; do ln -s /bin/true "/mock/bin/$name"; done
chmod +x /mock/bin/systemctl /mock/bin/curl /opt/cubie/bin/livekit-server
export PATH=/mock/bin:$PATH
python3 - <<'PY'
import os, sqlite3
db = sqlite3.connect('/var/lib/quipcall/quipcall.db')
db.execute('PRAGMA journal_mode=WAL')
db.execute('CREATE TABLE accounts(name)')
db.execute("INSERT INTO accounts VALUES ('old guest')")
db.commit()
os._exit(0)  # Keep the committed record in the WAL.
PY
install_app() { sh /bundle/deploy/install.sh; }
must_fail() { if install_app; then echo 'Unexpected install success' >&2; exit 1; fi; }
legacy_active() {
    test -L /etc/nginx/sites-enabled/quipcall.com.conf
    for unit in quipcall-server quipcall-livekit; do
        systemctl is-active "$unit" && systemctl is-enabled "$unit" || exit 1
    done
}
touch /var/lib/quipcall/cubie.db
must_fail
legacy_active
rm /var/lib/quipcall/cubie.db

for failure in api nginx; do
    touch "/mock/fail-$failure"
    must_fail
    legacy_active
    test ! -e /var/lib/cubie
    rm "/mock/fail-$failure"
done

install_app
test ! -e /etc/nginx/sites-enabled/quipcall.com.conf
test "$(cat /var/lib/cubie/.from-quipcall)" = live
grep -q 'existing-key: existing-secret' /opt/cubie/secrets.env
python3 - <<'PY'
import sqlite3
with sqlite3.connect('/var/lib/cubie/cubie.db') as db:
    assert db.execute('SELECT name FROM accounts').fetchone() == ('old guest',)
    db.execute("INSERT INTO accounts VALUES ('new guest')")
PY

# Simulate an interruption after writing the new config and before restarting.
echo '# changed' >>/bundle/deploy/livekit.yaml
touch /mock/fail-daemon-reload
must_fail
rm /mock/fail-daemon-reload
: >/mock/service-log
install_app
grep -qx 'restart cubie-livekit' /mock/service-log
: >/mock/service-log
install_app
grep -qx 'start cubie-livekit' /mock/service-log
python3 - <<'PY'
import sqlite3
with sqlite3.connect('/var/lib/cubie/cubie.db') as db:
    assert db.execute('SELECT COUNT(*) FROM accounts').fetchone() == (2,)
PY
echo 'Cutover, rollback, and retry checks passed.'
CONTAINER

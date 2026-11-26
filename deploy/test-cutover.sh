#!/bin/sh
# Exercise the first production cutover using a disposable Linux filesystem.
set -eu
cd "$(dirname "$0")/.."
docker run --rm -i --network none -v "$PWD/deploy:/source:ro" python:3.11-slim-bookworm sh -se <<'CONTAINER'
mkdir -p /bundle/web /etc/sudoers.d /opt/cubie/bin /opt/quipcall /var/lib/quipcall /etc/nginx/sites-enabled /etc/nginx/sites-available /etc/systemd/system /etc/letsencrypt/live/cubiecall.com /mock/bin /mock/units
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
        reload) test ! -e /mock/fail-nginx || exit 1; echo reload >> /mock/service-log ;;
        *) exit 1 ;;
    esac
done
if [ "$action" = daemon-reload ]; then test ! -e /mock/fail-daemon-reload; fi
SH
cat >/mock/bin/curl <<'SH'
#!/bin/sh
case "$*" in *4601/api/health*) test ! -e /mock/fail-api ;; *) exit 0 ;; esac
SH
cat >/mock/bin/mv <<'SH'
#!/bin/sh
/bin/mv "$@" || exit 1
if [ "$2" = /var/lib/cubie/.from-quipcall ] && [ -e /mock/kill-at-publishing ]; then
    kill -KILL "$PPID"
fi
SH
chmod +x /mock/bin/mv
cat >/mock/bin/certbot <<'SH'
#!/bin/sh
test ! -e /mock/fail-certbot || exit 1
args=" $* "
for host in cubiecall.com cubycall.com quipcall.com www.cubiecall.com www.quipcall.com; do
    case "$args" in *" -d $host "*) ;; *) exit 1 ;; esac
done
case "$args" in *" -d www.cubycall.com "*) exit 1 ;; esac
SH
cat >/mock/bin/getent <<'SH'
#!/bin/sh
case "$*" in 'ahosts www.cubycall.com') exit 2 ;; 'ahosts www.'*) exit 0 ;; *) exec /usr/bin/getent "$@" ;; esac
SH
chmod +x /mock/bin/getent
chmod +x /mock/bin/certbot
for name in nginx sleep journalctl visudo; do ln -s /bin/true "/mock/bin/$name"; done
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

for failure in certbot nginx; do
    touch /mock/kill-at-publishing
    must_fail
    rm /mock/kill-at-publishing
    test "$(cat /var/lib/cubie/.from-quipcall)" = publishing
    touch "/mock/fail-$failure"
    must_fail
    legacy_active
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
# An existing site needs the trusted proxy headers before the API restarts.
test "$(head -n 1 /mock/service-log)" = reload
grep -qx 'restart cubie-server' /mock/service-log
test ! -e /opt/cubie/.cutover-pending
python3 - <<'PY'
import sqlite3
with sqlite3.connect('/var/lib/cubie/cubie.db') as db:
    assert db.execute('SELECT COUNT(*) FROM accounts').fetchone() == (2,)
PY

# A routine deploy keeps what it replaced: the server, and the database as it
# was. A retry after a failed deploy must not replace that with the failed
# attempt's own state.
rows() {
    rm -rf /tmp/check && mkdir /tmp/check && cp "$1"/cubie.db* /tmp/check/
    python3 -c "import sqlite3; print(sqlite3.connect('/tmp/check/cubie.db').execute('SELECT COUNT(*) FROM accounts').fetchone()[0])"
}
add() { python3 -c "import sqlite3; db = sqlite3.connect('/var/lib/cubie/cubie.db'); db.execute(\"INSERT INTO accounts VALUES ('$1')\"); db.commit()"; }
test "$(stat -c %a /opt/cubie/previous)" = 700
test "$(rows /opt/cubie/previous)" = 2
echo 'server two' >/bundle/cubie-server
add 'before server two'
touch /mock/fail-api
must_fail
rm /mock/fail-api
test ! -s /opt/cubie/previous/cubie-server
test "$(rows /opt/cubie/previous)" = 3
add 'written by the failed attempt'
install_app
test ! -s /opt/cubie/previous/cubie-server
test "$(rows /opt/cubie/previous)" = 3
grep -qx 'server two' /opt/cubie/bin/cubie-server
# The deploy that finished is the next one's rollback point.
install_app
grep -qx 'server two' /opt/cubie/previous/cubie-server
test "$(rows /opt/cubie/previous)" = 4
# GitHub Actions' way in: an account that owns nothing and may run one thing.
grep -qx 'cubie-deploy ALL=(root) NOPASSWD: /opt/cubie/bin/cubie-app receive' /etc/sudoers.d/cubie-deploy
grep -q '^restrict,command="sudo -n /opt/cubie/bin/cubie-app receive" ssh-ed25519 ' /opt/cubie/ci/.ssh/authorized_keys
test "$(stat -c %U:%a /opt/cubie/ci/.ssh/authorized_keys /opt/cubie/bin/cubie-app | tr '\n' ' ')" = 'root:644 root:755 '
test "$(getent passwd cubie-deploy | cut -d: -f6)" = /opt/cubie/ci

# What it sends: the server, the web client and the commit they came from.
commit=0123456789abcdef0123456789abcdef01234567
send() { tar -czf - -C "$1" . | /opt/cubie/bin/cubie-app receive; }
refused() { if send "$1"; then echo 'Unexpected deployment' >&2; exit 1; fi; }
bundle() {
    rm -rf /ci && mkdir -p /ci/web/assets
    echo "$1" >/ci/cubie-server
    echo "$1" >/ci/web/index.html
    echo "$commit" >/ci/revision
}
echo 'web two' >/opt/cubie/web/index.html
bundle 'server three'
chmod 4777 /ci/cubie-server /ci/web/index.html
add 'before server three'
send /ci
grep -qx 'server three' /opt/cubie/bin/cubie-server
grep -qx 'server three' /opt/cubie/web/index.html
test "$(stat -c %U:%a /opt/cubie/bin/cubie-server /opt/cubie/web/index.html | tr '\n' ' ')" = 'root:755 root:644 '
test "$(cat /opt/cubie/.revision)" = "$commit"
grep -qx 'server two' /opt/cubie/previous/cubie-server
grep -qx 'web two' /opt/cubie/previous/web/index.html
test "$(rows /opt/cubie/previous)" = 5
test ! -e /opt/cubie/incoming

# The same build again is not a deployment: the copy of server two stays.
: >/mock/service-log
send /ci | grep -qx "$commit is already deployed"
test ! -s /mock/service-log
grep -qx 'server two' /opt/cubie/previous/cubie-server

# A server that doesn't answer is replaced by the one before it, with the
# web client and the database as they were.
bundle 'server four'
add 'before server four'
# The database's directory is the server's own, and this server is hostile:
# it leaves a link there for root to write through.
echo untouched >/root-only
chmod 600 /root-only
cat >/mock/bin/curl <<'SH'
#!/bin/sh
case "$*" in *4601/api/health*)
    grep -qx 'server four' /opt/cubie/bin/cubie-server || exit 0
    ln -sf /root-only /var/lib/cubie/cubie.db
    exit 1 ;;
esac
SH
refused /ci 2>/tmp/refused
grep -q 'are back' /tmp/refused
grep -qx 'server three' /opt/cubie/bin/cubie-server
grep -qx 'server three' /opt/cubie/web/index.html
test "$(cat /opt/cubie/.revision)" = "$commit"
grep -qx untouched /root-only
test ! -L /var/lib/cubie/cubie.db
test "$(stat -c %U:%a /var/lib/cubie/cubie.db)" = cubie:600
test "$(rows /var/lib/cubie)" = 6
# And can be sent again once it's fixed, from the same rollback point.
add 'after the failed server four'
cat >/mock/bin/curl <<'SH'
#!/bin/sh
exit 0
SH
four=fedcba9876543210fedcba9876543210fedcba98
echo "$four" >/ci/revision
send /ci
grep -qx 'server four' /opt/cubie/bin/cubie-server
test "$(cat /opt/cubie/.revision)" = "$four"
grep -qx 'server three' /opt/cubie/previous/cubie-server
test "$(rows /opt/cubie/previous)" = 7

# Going back by hand is the same step.
add 'lost by going back'
/opt/cubie/bin/cubie-app back
grep -qx 'server three' /opt/cubie/bin/cubie-server
test "$(cat /opt/cubie/.revision)" = "$commit"
test "$(rows /var/lib/cubie)" = 7
# And what went back is what the next deploy keeps, as it is by then.
add 'after going back'
send /ci
grep -qx 'server four' /opt/cubie/bin/cubie-server
grep -qx 'server three' /opt/cubie/previous/cubie-server
test "$(rows /opt/cubie/previous)" = 8

# Nothing but the three, as plain files, changes anything.
bundle 'server five'
ln -s /etc/passwd /ci/web/passwd
refused /ci
rm /ci/web/passwd
touch /ci/install.sh
refused /ci
rm /ci/install.sh
echo main >/ci/revision
refused /ci
echo "$commit" >/ci/revision
rm /ci/web/index.html
refused /ci
if head -c 4096 /dev/urandom | /opt/cubie/bin/cubie-app receive; then exit 1; fi
if /opt/cubie/bin/cubie-app receive </dev/null; then exit 1; fi
if /opt/cubie/bin/cubie-app receive anything </dev/null; then exit 1; fi
# A connection that sends nothing doesn't hold the next deployment up for
# good, and what a killed attempt left goes with the next one.
sed 's/^WAIT=.*/WAIT=1/' /opt/cubie/bin/cubie-app >/tmp/impatient
mkfifo /tmp/silence
mkdir /opt/cubie/incoming
touch /opt/cubie/incoming/left
if sh /tmp/impatient receive <>/tmp/silence; then exit 1; fi
test ! -e /opt/cubie/incoming
touch /opt/cubie/.cutover-pending
bundle 'server five'
refused /ci
rm /opt/cubie/.cutover-pending
grep -qx 'server four' /opt/cubie/bin/cubie-server
grep -qx 'server three' /opt/cubie/previous/cubie-server
test ! -e /opt/cubie/incoming

# A deploy from the release Mac names no commit: the last one sent mustn't
# pass for it, now or in the copy the next deploy keeps.
test -s /opt/cubie/.revision
install_app
grep -qx 'server two' /opt/cubie/bin/cubie-server
test ! -e /opt/cubie/.revision
bundle 'server six'
send /ci
test ! -e /opt/cubie/previous/revision
echo 'Cutover, rollback, retry, and GitHub Actions deployment checks passed.'
CONTAINER

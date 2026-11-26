#!/bin/sh
# Deploy the server, web client and VPS config: `make deploy`.
# Builds here, uploads one bundle, runs deploy/install.sh on the box.
set -eu
cd "$(dirname "$0")/.."
. ./deploy/site-env.sh
HOST=${CUBIE_HOST:-${QUIPCALL_HOST:?set CUBIE_HOST to the deployment SSH address}}
bundle=$(mktemp -d)
trap 'rm -rf "$bundle"' EXIT
mkdir -p "$bundle/deploy"
python3 deploy/render-site.py "$bundle/deploy/rendered"
. "$bundle/deploy/rendered/settings.env"

# SQLite and rustls include C code: Zig supplies the musl cross toolchain.
command -v zig >/dev/null || { echo 'Install Zig before deploying (brew install zig)' >&2; exit 1; }
command -v cargo-zigbuild >/dev/null || { echo 'Install cargo-zigbuild before deploying (cargo install cargo-zigbuild --locked)' >&2; exit 1; }
cargo zigbuild --release -p cubie-server --target x86_64-unknown-linux-musl
(cd apps/web && pnpm install --frozen-lockfile --silent &&
    VITE_CUBIE_PUBLIC_URL="$PUBLIC_URL" VITE_CUBIE_LEGACY_URL="$LEGACY_URL" pnpm build)

cp target/x86_64-unknown-linux-musl/release/cubie-server "$bundle/"
cp -R apps/web/dist "$bundle/web"
cp -R deploy/install.sh deploy/app.sh deploy/ci-access.sh deploy/configure-secrets.py deploy/livekit.yaml deploy/systemd "$bundle/deploy/"
if [ -n "${CUBIE_CI_PUBLIC_KEY_FILE:-}" ]; then
    install -m 644 "$CUBIE_CI_PUBLIC_KEY_FILE" "$bundle/deploy/ci-deploy.pub"
fi

# Auth settings are optional on later deploys; values never appear in logs.
auth_file=${CUBIE_AUTH_ENV:-${QUIPCALL_AUTH_ENV:-deploy/auth.env.local}}
if [ -f "$auth_file" ]; then
    install -m 600 "$auth_file" "$bundle/deploy/auth.env"
fi

# No macOS metadata: GNU tar on the box would warn about every file.
COPYFILE_DISABLE=1 tar --no-xattrs --no-mac-metadata -C "$bundle" -czf - . |
	ssh "$HOST" 'set -eu
        umask 077
        stage=$(mktemp -d /opt/cubie-incoming.XXXXXX)
        trap "rm -rf -- \"$stage\"" EXIT
        tar -xzf - -C "$stage"
        status=0
        flock /run/lock/cubie-deploy.lock sh "$stage/deploy/install.sh" >"$stage/install.log" 2>&1 || status=$?
        cat "$stage/install.log"
        exit "$status"'

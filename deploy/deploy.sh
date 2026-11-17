#!/bin/sh
# Deploy the server, web client and VPS config: `make deploy`.
# Builds here, uploads one bundle, runs deploy/install.sh on the box.
set -eu
cd "$(dirname "$0")/.."
HOST=${CUBIE_HOST:-${QUIPCALL_HOST:-root@testron.dev}}

# SQLite and rustls include C code: Zig supplies the musl cross toolchain.
command -v zig >/dev/null || { echo 'Install Zig before deploying (brew install zig)' >&2; exit 1; }
command -v cargo-zigbuild >/dev/null || { echo 'Install cargo-zigbuild before deploying (cargo install cargo-zigbuild --locked)' >&2; exit 1; }
cargo zigbuild --release -p cubie-server --target x86_64-unknown-linux-musl
(cd apps/web && pnpm install --frozen-lockfile --silent && pnpm build)

bundle=$(mktemp -d)
trap 'rm -rf "$bundle"' EXIT
cp target/x86_64-unknown-linux-musl/release/cubie-server "$bundle/"
cp -R apps/web/dist "$bundle/web"
mkdir "$bundle/deploy"
cp -R deploy/install.sh deploy/configure-secrets.py deploy/livekit.yaml deploy/nginx deploy/systemd deploy/download "$bundle/deploy/"

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

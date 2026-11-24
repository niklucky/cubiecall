#!/bin/sh
# Lets GitHub Actions deploy the server and web client (docs/DEPLOY.md).
# Runs on the VPS as root, from deploy/install.sh. Safe to re-run.
#
# The workflow's SSH key (deploy/ci-deploy.pub; its private half is a GitHub
# secret) signs in as `cubie-deploy`. That account gets no shell: whatever a
# connection asks for, it runs `cubie-app receive` as root (deploy/app.sh),
# and sudo allows it nothing else. It owns no file, its own authorized_keys
# included, so it can't change what its key may do.
set -eu
umask 022
here=$(cd "$(dirname "$0")" && pwd)
ROOT=/opt/cubie
ACCOUNT=cubie-deploy
HOME_DIR=$ROOT/ci
SUDOERS=/etc/sudoers.d/$ACCOUNT

# One bare public key: options in front of it would be the key's own say in
# what it may do.
key=$(cat "$here/ci-deploy.pub")
printf '%s\n' "$key" | grep -Eqx 'ssh-ed25519 [A-Za-z0-9+/=]+( [A-Za-z0-9@._-]+)?' || {
    echo 'deploy/ci-deploy.pub must be one ssh-ed25519 public key.' >&2
    exit 1
}
[ -d /etc/sudoers.d ] || {
    echo 'sudo is not installed.' >&2
    exit 1
}

install -m 755 "$here/app.sh" "$ROOT/bin/cubie-app.new"
mv "$ROOT/bin/cubie-app.new" "$ROOT/bin/cubie-app"

id "$ACCOUNT" >/dev/null 2>&1 ||
    useradd --system --home-dir "$HOME_DIR" --no-create-home --shell /bin/sh "$ACCOUNT"
mkdir -p "$HOME_DIR/.ssh"
chown root:root "$HOME_DIR" "$HOME_DIR/.ssh"
chmod 755 "$HOME_DIR" "$HOME_DIR/.ssh"
printf 'restrict,command="sudo -n %s/bin/cubie-app receive" %s\n' "$ROOT" "$key" > "$HOME_DIR/.ssh/authorized_keys.new"
chmod 644 "$HOME_DIR/.ssh/authorized_keys.new"
mv "$HOME_DIR/.ssh/authorized_keys.new" "$HOME_DIR/.ssh/authorized_keys"

# The box is shared, and a sudoers file sudo can't read locks everyone out:
# check the rule alone first, then everything with it in place. sudo ignores
# file names with a dot, so the candidate isn't live while it's checked.
printf '%s ALL=(root) NOPASSWD: %s/bin/cubie-app receive\n' "$ACCOUNT" "$ROOT" > "$SUDOERS.new"
chmod 440 "$SUDOERS.new"
visudo -cf "$SUDOERS.new" >/dev/null || {
    rm -f "$SUDOERS.new"
    echo 'The sudo rule for deployments was refused; nothing changed.' >&2
    exit 1
}
if ! cmp -s "$SUDOERS.new" "$SUDOERS"; then
    mv "$SUDOERS.new" "$SUDOERS"
    visudo -c >/dev/null || {
        rm -f "$SUDOERS"
        echo 'sudo refused its configuration with the deployment rule; the rule was removed.' >&2
        exit 1
    }
    echo "updated $SUDOERS"
fi
rm -f "$SUDOERS.new"

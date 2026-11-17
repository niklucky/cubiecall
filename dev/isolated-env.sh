# Source from the repository root in bash or zsh. Does not start services.
# Always allocate fresh state; never source dev.env for an isolated test stack.
export CUBIE_TEST_DIR="$(mktemp -d /tmp/cubie-test.XXXXXX)"
export CUBIE_BIND=127.0.0.1:8180
export CUBIE_PUBLIC_URL=http://127.0.0.1:5273
export CUBIE_API=http://127.0.0.1:8180
export CUBIE_DATABASE_URL="sqlite://$CUBIE_TEST_DIR/cubie.db"
export CUBIE_MAIL_DIR="$CUBIE_TEST_DIR/mail"
export CUBIE_BOOTSTRAP_EMAIL=admin@example.test
export CUBIE_DATA_DIR="$CUBIE_TEST_DIR/desktop"
export CUBIE_NO_MIC=1 CUBIE_FAKE_CAMERA=1 CUBIE_FAKE_SCREEN=1
export LIVEKIT_PUBLIC_URL=ws://127.0.0.1:7980
export LIVEKIT_API_URL=http://127.0.0.1:7980
export LIVEKIT_API_KEY=cubie-dev
export LIVEKIT_API_SECRET=cubie-dev-secret-0123456789abcdef
export CUBIE_LIVEKIT_CONFIG="$CUBIE_TEST_DIR/livekit.yaml"
cat > "$CUBIE_LIVEKIT_CONFIG" <<'YAML'
port: 7980
bind_addresses: [127.0.0.1]
rtc:
  tcp_port: 7981
  udp_port: 7982
  use_external_ip: false
keys:
  cubie-dev: cubie-dev-secret-0123456789abcdef
logging:
  level: info
YAML

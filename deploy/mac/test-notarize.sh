#!/bin/sh
# Exercise release gating without contacting Apple or using a signing key.
set -eu
notarize=$(cd "$(dirname "$0")" && pwd)/notarize.sh
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir "$tmp/bin"
cat >"$tmp/bin/mock" <<'SH'
#!/bin/sh
set -eu
tool=${0##*/}
printf '%s %s\n' "$tool" "$*" >>"$TRACE_FILE"
case "$tool" in
    ditto)
        if [ -f "$4/ticket" ]; then echo stapled; else echo unstapled; fi >"$5"
        ;;
    xcrun)
        case "$1 $2" in
            'notarytool submit')
                case "$MOCK_SCENARIO" in
                    invalid) echo '{"id":"submission-id","status":"Invalid"}' ;;
                    pending) echo '{"id":"submission-id","status":"In Progress"}'; exit 1 ;;
                    network) exit 1 ;;
                    missing-id) echo '{"status":"Accepted"}' ;;
                    *) echo '{"id":"submission-id","status":"Accepted"}' ;;
                esac
                ;;
            'notarytool log')
                [ "$MOCK_SCENARIO" != log-failure ] || exit 1
                echo '{"issues":null}' >"$6"
                ;;
            'stapler staple')
                [ "$MOCK_SCENARIO" != staple-failure ] || exit 1
                touch "$3/ticket"
                ;;
            'stapler validate') [ "$MOCK_SCENARIO" != validation-failure ] ;;
            *) exit 1 ;;
        esac
        ;;
    codesign) [ "$MOCK_SCENARIO" != signature-failure ] ;;
    spctl) [ "$MOCK_SCENARIO" != gatekeeper-failure ] ;;
    *) exit 1 ;;
esac
SH
chmod +x "$tmp/bin/mock"
for tool in ditto xcrun codesign spctl; do ln -s mock "$tmp/bin/$tool"; done

for scenario in invalid pending network missing-id log-failure staple-failure validation-failure signature-failure gatekeeper-failure accepted; do
    out="$tmp/$scenario with spaces"
    mkdir -p "$out/Cubie.app"
    result=0
    MOCK_SCENARIO=$scenario TRACE_FILE="$out/trace" PATH="$tmp/bin:$PATH" \
        sh "$notarize" "$out/Cubie.app" "$out/Cubie.zip" cubie-notary >"$out/output" 2>&1 || result=$?
    if [ "$scenario" = accepted ]; then
        [ "$result" = 0 ]
        [ "$(cat "$out/Cubie.zip")" = stapled ]
        test -s "$out/Cubie-notary-log.json"
        test ! -e "$out/Cubie-submission.zip"
    else
        [ "$result" != 0 ]
        test ! -e "$out/Cubie.zip"
    fi
    case "$scenario" in
        invalid|pending)
            test -s "$out/Cubie-notary-log.json"
            if grep -q 'stapler' "$out/trace"; then
                echo "Stapled an unaccepted submission: $scenario" >&2
                exit 1
            fi
            ;;
    esac
done
echo 'Mac notarization gating tests passed.'

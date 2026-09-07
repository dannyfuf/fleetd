#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname "$0")/../.." && pwd)
test_root=$(mktemp -d "${TMPDIR:-/tmp}/fleet-bootstrap-zig-test.XXXXXX")
trap 'rm -rf "$test_root"' EXIT HUP INT TERM
test_home="$test_root/home"
fake_bin="$test_root/bin"
sdk="$test_root/MacOSX.sdk"
install_dir="$test_home/.local/zig-0.15.2"
/bin/mkdir -p "$fake_bin" "$sdk/usr/lib" "$sdk/usr/include" "$install_dir"

printf '%s\n' '#!/bin/sh' 'case "$1" in' \
    '-s) printf "%s\n" Darwin ;;' \
    '-m) printf "%s\n" arm64 ;;' \
    '*) exit 2 ;;' \
    'esac' > "$fake_bin/uname"
/bin/chmod +x "$fake_bin/uname"
printf '%s\n' '#!/bin/sh' 'printf "%s\n" 0.15.2' > "$install_dir/zig"
/bin/chmod +x "$install_dir/zig"

printf '%s\n' '{"Version":"first"}' > "$sdk/SDKSettings.json"
printf '%s\n' 'targets: [ arm64e-macos ]' 'install-name: first' > "$sdk/usr/lib/libSystem.tbd"
PATH="$fake_bin:$PATH" HOME="$test_home" FLEET_MACOS_SDK_PATH="$sdk" \
    "$repo_root/scripts/bootstrap-zig.sh" >/dev/null

[ -L "$install_dir/macos-sdk-arm64" ]
first_target=$(/usr/bin/readlink "$install_dir/macos-sdk-arm64")
/usr/bin/grep -q 'arm64-macos' "$install_dir/macos-sdk-arm64/usr/lib/libSystem.tbd"
if /usr/bin/grep -q 'arm64e-macos' "$install_dir/macos-sdk-arm64/usr/lib/libSystem.tbd"; then
    printf '%s\n' 'initial overlay still contains arm64e target' >&2
    exit 1
fi

printf '%s\n' '{"Version":"second"}' > "$sdk/SDKSettings.json"
printf '%s\n' 'targets: [ arm64e-macos ]' 'install-name: second' > "$sdk/usr/lib/libSystem.tbd"
PATH="$fake_bin:$PATH" HOME="$test_home" FLEET_MACOS_SDK_PATH="$sdk" \
    "$repo_root/scripts/bootstrap-zig.sh" >/dev/null

second_target=$(/usr/bin/readlink "$install_dir/macos-sdk-arm64")
if [ "$first_target" = "$second_target" ]; then
    printf '%s\n' 'SDK fingerprint did not replace the overlay after an upgrade' >&2
    exit 1
fi
/usr/bin/grep -q 'install-name: second' "$install_dir/macos-sdk-arm64/usr/lib/libSystem.tbd"
/usr/bin/grep -q '"Version":"second"' "$install_dir/macos-sdk-arm64/SDKSettings.json"

printf '%s\n' 'sdk_upgrade_replaces_overlay: passed'

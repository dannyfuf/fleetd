#!/bin/sh
set -eu

zig_version=0.15.2
zig_sha256=3cc2bab367e185cdfb27501c4b30b1b0653c28d9f73df8dc91488e66ece5fa6b
zig_url="https://ziglang.org/download/$zig_version/zig-aarch64-macos-$zig_version.tar.xz"
install_dir="$HOME/.local/zig-$zig_version"
shim="$HOME/.cargo/bin/zig"
script_dir=$(CDPATH= cd -- "$(dirname "$0")" && pwd)
temporary=
overlay_tmp=
overlay_link_tmp=
overlay_legacy=
fingerprint_input=

cleanup() {
    if [ -n "$temporary" ] && [ -d "$temporary" ]; then
        /bin/rm -rf "$temporary"
    fi
    if [ -n "$overlay_tmp" ] && [ -d "$overlay_tmp" ]; then
        /bin/rm -rf "$overlay_tmp"
    fi
    if [ -n "$overlay_link_tmp" ] && [ -L "$overlay_link_tmp" ]; then
        /bin/rm "$overlay_link_tmp"
    fi
    if [ -n "$overlay_legacy" ] && [ -d "$overlay_legacy" ]; then
        /bin/rm -rf "$overlay_legacy"
    fi
    if [ -n "$fingerprint_input" ] && [ -f "$fingerprint_input" ]; then
        /bin/rm "$fingerprint_input"
    fi
}
trap cleanup EXIT HUP INT TERM

if [ "$(uname -s)" != Darwin ] || [ "$(uname -m)" != arm64 ]; then
    printf '%s\n' "This bootstrap installs Zig $zig_version for aarch64-macos only." >&2
    exit 1
fi

if [ -x "$install_dir/zig" ]; then
    installed_version=$("$install_dir/zig" version)
    if [ "$installed_version" != "$zig_version" ]; then
        printf 'Refusing to replace %s (reports version %s).\n' "$install_dir" "$installed_version" >&2
        exit 1
    fi
else
    if [ -e "$install_dir" ]; then
        printf 'Refusing to replace existing incomplete install at %s.\n' "$install_dir" >&2
        exit 1
    fi
    temporary=$(mktemp -d "${TMPDIR:-/tmp}/fleet-zig.XXXXXX")
    archive="$temporary/zig.tar.xz"
    /usr/bin/curl -fL "$zig_url" -o "$archive"
    printf '%s  %s\n' "$zig_sha256" "$archive" | /usr/bin/shasum -a 256 -c -
    /usr/bin/tar -xf "$archive" -C "$temporary"
    /bin/mkdir -p "$HOME/.local"
    /bin/mv "$temporary/zig-aarch64-macos-$zig_version" "$install_dir"
fi

/bin/mkdir -p "$install_dir/wrappers" "$HOME/.cargo/bin" "$HOME/.cache/zig"
/usr/bin/install -m 755 "$script_dir/zig-wrapper.sh" "$install_dir/wrappers/zig"
/usr/bin/install -m 755 "$script_dir/xcrun-wrapper.sh" "$install_dir/wrappers/xcrun"

sdk_path=${FLEET_MACOS_SDK_PATH:-$(/usr/bin/xcrun --sdk macosx --show-sdk-path)}
sdk_overlay="$install_dir/macos-sdk-arm64"
if /usr/bin/grep -q 'arm64e-macos' "$sdk_path/usr/lib/libSystem.tbd"; then
    fingerprint_input=$(mktemp "${TMPDIR:-/tmp}/fleet-sdk-fingerprint.XXXXXX")
    {
        printf 'sdk=%s\n' "$sdk_path"
        /usr/bin/shasum -a 256 "$sdk_path/SDKSettings.json"
        /usr/bin/find "$sdk_path/usr/lib" -type f -name '*.tbd' | LC_ALL=C /usr/bin/sort | while IFS= read -r source; do
            relative=${source#"$sdk_path/"}
            printf 'file=%s\n' "$relative"
            /usr/bin/shasum -a 256 "$source"
        done
        /usr/bin/find "$sdk_path/usr/lib" -type l -name '*.tbd' | LC_ALL=C /usr/bin/sort | while IFS= read -r source; do
            relative=${source#"$sdk_path/"}
            printf 'link=%s:%s\n' "$relative" "$(/usr/bin/readlink "$source")"
        done
    } > "$fingerprint_input"
    sdk_fingerprint=$(/usr/bin/shasum -a 256 "$fingerprint_input")
    sdk_fingerprint=${sdk_fingerprint%% *}
    /bin/rm "$fingerprint_input"
    fingerprint_input=
    installed_fingerprint=
    if [ -f "$sdk_overlay/.fleet-sdk-fingerprint" ]; then
        installed_fingerprint=$(/bin/cat "$sdk_overlay/.fleet-sdk-fingerprint")
    fi
    if [ "$installed_fingerprint" != "$sdk_fingerprint" ] || \
        [ ! -f "$sdk_overlay/usr/lib/libSystem.tbd" ] || \
        /usr/bin/grep -q 'arm64e-macos' "$sdk_overlay/usr/lib/libSystem.tbd"; then
        overlay_tmp=$(mktemp -d "$install_dir/.macos-sdk-arm64.XXXXXX")
        /bin/mkdir -p "$overlay_tmp/usr/lib"
        /bin/cp "$sdk_path/SDKSettings.json" "$overlay_tmp/SDKSettings.json"
        /usr/bin/find "$sdk_path/usr/lib" -type f -name '*.tbd' | while IFS= read -r source; do
            relative=${source#"$sdk_path/"}
            destination="$overlay_tmp/$relative"
            /bin/mkdir -p "$(dirname "$destination")"
            /usr/bin/sed \
                -e 's/arm64e-macos/arm64-macos/g' \
                -e 's/arm64e-maccatalyst/arm64-maccatalyst/g' \
            "$source" > "$destination"
        done
        /usr/bin/find "$sdk_path/usr/lib" -type l -name '*.tbd' | while IFS= read -r source; do
            relative=${source#"$sdk_path/"}
            destination="$overlay_tmp/$relative"
            /bin/mkdir -p "$(dirname "$destination")"
            /bin/ln -s "$(/usr/bin/readlink "$source")" "$destination"
        done
        /bin/ln -s "$sdk_path/usr/include" "$overlay_tmp/usr/include"
        printf '%s\n' "$sdk_fingerprint" > "$overlay_tmp/.fleet-sdk-fingerprint"

        overlay_version="$install_dir/macos-sdk-arm64-$sdk_fingerprint"
        if [ -d "$overlay_version" ]; then
            /bin/rm -rf "$overlay_tmp"
        else
            /bin/mv "$overlay_tmp" "$overlay_version"
        fi
        overlay_tmp=

        overlay_link_tmp="$install_dir/.macos-sdk-arm64-link.$$"
        /bin/rm -f "$overlay_link_tmp"
        /bin/ln -s "$(basename "$overlay_version")" "$overlay_link_tmp"
        if [ -L "$sdk_overlay" ] || [ ! -e "$sdk_overlay" ]; then
            /bin/mv -fh "$overlay_link_tmp" "$sdk_overlay"
        else
            overlay_legacy="$install_dir/.macos-sdk-arm64-legacy.$$"
            /bin/mv "$sdk_overlay" "$overlay_legacy"
            if /bin/mv "$overlay_link_tmp" "$sdk_overlay"; then
                /bin/rm -rf "$overlay_legacy"
            else
                /bin/mv "$overlay_legacy" "$sdk_overlay"
                exit 1
            fi
        fi
        overlay_link_tmp=
        overlay_legacy=
    fi
fi

expected_shim="$install_dir/wrappers/zig"
if [ -e "$shim" ] || [ -L "$shim" ]; then
    current_shim=$(/usr/bin/readlink "$shim" 2>/dev/null || true)
    if [ "$current_shim" != "$expected_shim" ]; then
        printf 'Refusing to replace existing Zig shim at %s.\n' "$shim" >&2
        exit 1
    fi
else
    /bin/ln -s "$expected_shim" "$shim"
fi

verified_version=$("$install_dir/zig" version)
shim_version=$("$shim" version)
if [ "$verified_version" != "$zig_version" ] || [ "$shim_version" != "$zig_version" ]; then
    printf '%s\n' "Zig verification failed." >&2
    exit 1
fi

printf 'Installed Zig %s at %s.\n' "$zig_version" "$install_dir"
printf 'Verified %s and %s both report %s.\n' "$install_dir/zig" "$shim" "$zig_version"
printf '%s\n' 'For shells without ~/.cargo/bin on PATH:'
printf '%s\n' 'export PATH="$HOME/.cargo/bin:$PATH"'

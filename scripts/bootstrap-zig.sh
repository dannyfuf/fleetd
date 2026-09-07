#!/bin/sh
set -eu

zig_version=0.15.2
zig_sha256=3cc2bab367e185cdfb27501c4b30b1b0653c28d9f73df8dc91488e66ece5fa6b
zig_url="https://ziglang.org/download/$zig_version/zig-aarch64-macos-$zig_version.tar.xz"
install_dir="$HOME/.local/zig-$zig_version"
shim="$HOME/.cargo/bin/zig"
script_dir=$(CDPATH= cd -- "$(dirname "$0")" && pwd)

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
    trap 'rm -rf "$temporary"' EXIT HUP INT TERM
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

sdk_path=$(/usr/bin/xcrun --sdk macosx --show-sdk-path)
sdk_overlay="$install_dir/macos-sdk-arm64"
if /usr/bin/grep -q 'arm64e-macos' "$sdk_path/usr/lib/libSystem.tbd"; then
    if [ ! -f "$sdk_overlay/usr/lib/libSystem.tbd" ] || \
        /usr/bin/grep -q 'arm64e-macos' "$sdk_overlay/usr/lib/libSystem.tbd"; then
        overlay_tmp="$install_dir/.macos-sdk-arm64.tmp"
        /bin/rm -rf "$overlay_tmp"
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
        /bin/rm -rf "$sdk_overlay"
        /bin/mv "$overlay_tmp" "$sdk_overlay"
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

#!/bin/sh
set -eu

script_path=$0
while [ -L "$script_path" ]; do
    link=$(/usr/bin/readlink "$script_path")
    case "$link" in
        /*) script_path=$link ;;
        *) script_path=$(dirname "$script_path")/$link ;;
    esac
done
zig_home=$(CDPATH= cd -- "$(dirname "$script_path")/.." && pwd)
real_zig="$zig_home/zig"
sdk_overlay="$zig_home/macos-sdk-arm64"
wrapper_dir="$zig_home/wrappers"

if [ "${1:-}" = "build" ] && [ -d "$sdk_overlay" ]; then
    shift
    PATH="$wrapper_dir:$PATH" exec "$real_zig" build --sysroot "$sdk_overlay" "$@"
fi

exec "$real_zig" "$@"

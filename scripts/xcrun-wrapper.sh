#!/bin/sh
set -eu

zig_home=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
case " $* " in
    *" --show-sdk-path "*) printf '%s\n' "$zig_home/macos-sdk-arm64" ;;
    *) exec /usr/bin/xcrun "$@" ;;
esac

# Fleet development

Fleet is a Cargo workspace built with Rust 1.97.1 and edition 2024. The repository's
`rust-toolchain.toml` selects the minimal 1.97.1 toolchain and installs `rustfmt` and `clippy`, so
ordinary `cargo` commands use the correct compiler automatically.

## Common commands

Run the project commands through the checked-in `Makefile`:

```sh
make check       # type-check every workspace crate
make build       # build every workspace crate
make run-app     # launch the native Fleet app
make run-daemon  # run the fleetd stub
make test        # test every workspace crate
make fmt         # format the workspace
make clippy      # lint all targets and features with warnings denied
```

Direct Cargo equivalents work as usual. Build artifacts use Cargo's default target directory,
`target/` inside this repository. Sharing that repository-local directory between commands in
the same worktree is supported. Do not point multiple worktrees at one external
`CARGO_TARGET_DIR`: Cargo's relative dep-info paths can make one worktree accept another's stale
artifacts. Give parallel worktrees separate target directories when an override is necessary.

## Zig

Building `fleet-term` with its default `ghostty` feature requires Zig **0.15.2**. The pinned
`libghostty-vt 0.2.1` dependency builds its vendored Ghostty commit with that exact minimum
version. On Apple Silicon macOS, install and verify the official release with:

```sh
scripts/bootstrap-zig.sh
```

The idempotent bootstrap verifies the published SHA-256 checksum, installs the official binary at
`~/.local/zig-0.15.2/zig`, and exposes it as `~/.cargo/bin/zig`. The published
`libghostty-vt-sys 0.2.1` build script invokes `zig` by name and does not honor a `ZIG` executable
variable, so `~/.cargo/bin` must be on `PATH` (the bootstrap prints the required export).

The repository config directs Zig's global cache to `~/.cache/zig`. On macOS 26 with the Xcode 26
SDK, the bootstrap also creates a private SDK overlay below the Zig installation that maps the
SDK's `arm64e-macos` text-based stubs to the `arm64-macos` target expected by Zig 0.15.2. The real
Xcode SDK is not modified.

## Crate map

| Crate | Responsibility |
| --- | --- |
| `fleet-core` | Pure domain types, schemas, validation, and helpers. |
| `fleet-proto` | Client/daemon wire messages, framing, terminal updates, and socket paths. |
| `fleet-term` | Daemon-owned PTYs, VT engines, terminal host, and key encoding. |
| `fleet-daemon` | Stores, adapters, services, jobs, and the `fleetd` socket server. |
| `fleet-client` | Async daemon connection, spawning, request APIs, events, and terminal attach. |
| `fleet-ui-kit` | Domain-independent GPUI theme, assets, icons, and components. |
| `fleet-cli` | Clap commands plus JSON and human output. |
| `fleet-app` | The `fleet` GPUI app, state mirror, screens, dialogs, and terminal rendering. |

## Parallel ownership rule

Only edit files in your assigned module. Never edit `lib.rs` or `mod.rs` except to add `pub use`
re-exports of public items from your own module. The complete module tree is predeclared so agents
can implement separate files without creating shared-file conflicts. Because `cargo fmt -p` still
formats an entire crate, parallel work should run `rustfmt` on owned files and leave the workspace
wide `cargo fmt --all` pass to integration.

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
`target/` inside this repository. Do not set a shared external `CARGO_TARGET_DIR`; keeping each
worktree's artifacts local avoids collisions between parallel agents.

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
can implement separate files without creating shared-file conflicts.

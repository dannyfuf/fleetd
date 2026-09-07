# 0003 — Fleet builds its own design system

**Adopted.** `fleet-ui-kit` is written in-house. No third-party GPUI component library is a
dependency, and Zed's `crates/ui` is not vendored: it is GPL-3.0-or-later, while GPUI itself is
Apache-2.0. `gpui-component` / `gpui-kit` is additionally incompatible, because it pins its own
GPUI republish (see `0001-gpui-and-toolchain.md`).

Four patterns were mined from permissively licensed prior art rather than invented:

| Pattern | Shape in `fleet-ui-kit` |
| --- | --- |
| Semantic tokens | `theme::Tokens` — colors by role, radius, spacing, typography, metrics |
| Theme access | `ActiveTheme` trait over `App`, theme installed as a GPUI global |
| Motion | Fixed enter/exit/move durations and curves in `theme::Motion` |
| Icons | Embedded `AssetSource` + a typed icon enum resolved through `svg().path(..)` |

Consequences: views in `fleet-app` and `fleet-lazygit` compose kit components and do no ad-hoc
styling; every dimension, color, opacity and duration is a token. `docs/DESIGN-SYSTEM.md` is the
prose half of this contract and changes together with `crates/fleet-ui-kit/src/theme/tokens.rs`.

Provenance (removed from the tree; read them in git history): `docs/research/gpui.md` @ b5741b7.

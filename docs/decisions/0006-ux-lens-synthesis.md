# 0006 — The UX specification is a synthesis of three lenses

**Adopted.** `docs/UX-SPEC.md` is authoritative for what every screen shows. It was produced by
merging three competing whole-product proposals, each written to a single lens:

| Lens | What survived into the spec |
| --- | --- |
| Glanceability and minimalism | The base system: frame proportions, the status-glyph vocabulary, the token set, the closed-by-default detail panel, and the shared dialog frame |
| Background awareness and safety | The safety spine: the freshness ladder and stamps, inspection facts quoted verbatim in confirms, the jobs panel's cancel affordances, and the quit-with-running-work flow |
| Flow speed | The navigation, mode and toast layer: MRU session and terminal toggles, absolute `g`-prefixed destinations, filter-to-open, and the rule that no single key can quit |

Where the proposals disagreed, `UX-SPEC.md` decides; those decisions are marked **[D-n]** and
collected in its §8. Two of them resolved open questions the proposals had left standing: the
auto-inspect cadence (**[D-4]**) and the 100-row cap on the `All`-scope PR list (**[D-7]**).

The proposals' suggested key bindings were not adopted wholesale. `docs/KEYMAP.md` is the single
authority for keys; `crates/fleet-app/src/keymap.rs` is generated from the same table, and
`docs/APP-CONTRACTS.md` records the three places where implementation had to arbitrate against it.

Provenance (removed from the tree; read them in git history): `docs/ux/proposal-glanceability.md`
@ 3a296ee, `docs/ux/proposal-background-safety.md` @ 3a296ee, `docs/ux/proposal-flow-speed.md` @
06e856b.

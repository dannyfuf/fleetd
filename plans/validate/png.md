# Adversarial validation — hand-rolled PNG/inflate decoder (`crates/fleet-harness/src/baseline.rs`)

Base `881162c5c283cefb4319bbfdb79629ef31960745`, branch `test-harness`.
Both candidates were reproduced against the real decoder with a temporary `#[cfg(test)] mod
adversarial_tmp` appended to `baseline.rs`, which has been reverted (`git checkout --`; the file
is byte-identical to HEAD).

## Shared groundwork — what is actually enforced, and by whom

**Bounds that exist.** `decode` (`baseline.rs:633-667`) bounds every chunk against the file
length (`end <= bytes.len()`, `baseline.rs:643`) and gates each chunk on its CRC-32
(`baseline.rs:653`). `parse_header` (`baseline.rs:691-720`) rejects a zero dimension, an unknown
compression/filter method, interlacing, an unknown colour type and an illegal bit depth.
`Huffman::build` bounds code lengths to 15 and over-subscription (`baseline.rs:977-1006`);
`dynamic_codes` allocates at most `288 + 32` lengths; the back-reference distance is bounded
against `output.len()` (`baseline.rs:1120`); `Bits::take` is bounded by the input slice.

**Bounds that do not exist.** There is no maximum dimension, no maximum decompressed size, no
maximum file size, and no `MAX_*` constant anywhere in the crate — `grep -n "MAX\|limit"
baseline.rs` returns only `u16::MAX`/`u32::MAX` inside the encoder and the CRC seed. Nothing
upstream of `read_image` caps the file either: `read_image` is `std::fs::read` + `decode`
(`baseline.rs:594-598`).

**Who supplies the bytes.** Exactly one decode reads attacker-controlled bytes:
`compare`'s `read_image(baseline)` (`baseline.rs:385`), reached from
`Baselines::check` (`baseline.rs:334-370`) → `scenario.rs:832`, and only in the `virtual` lane
(`check` returns `Skipped` otherwise, `baseline.rs:341`). Every other decode reads the run's own
`grim` capture: `compare`'s `read_image(actual)`, and `differing_share`/`differing_share_within`
in `lane.rs:391,485`.

**Correction to both findings' reachability claim.** `update_baseline` (`baseline.rs:503-520`)
decodes `actual` — the *fresh capture* — and then `std::fs::copy`s over the baseline. It never
decodes the stored baseline, and `same_bytes` (`baseline.rs:522`) compares raw bytes without
decoding. So `--update-baselines` is **not** an attacker-reachable decode; F1-5's and F1-6's
citation of `baseline.rs:508` is wrong. The single attacker entry point is a plain
`make harness` run in the `virtual` lane against a committed baseline.

**Build profile — claim confirmed.** Root `Cargo.toml` declares only `[profile.dev] opt-level =
1` and `[profile.dev.package."*"]`; there is **no** `[profile.release]` section and **no**
`.cargo/config.toml` in the tree. `Makefile:10` sets `RELEASE ?= 0`, so `make harness` → `build`
→ `cargo build --workspace` with no `--release`, i.e. the `dev` profile, where `debug-assertions`
(and therefore `overflow-checks`) default to **on**. The repro panics under the `test` profile,
which inherits `dev` — the build output line reads `Finished \`test\` profile [optimized +
debuginfo]`, and the overflow check still fires.

**Existing malformed-input coverage.** Two tests only: `a_baseline_that_is_not_a_png_is_named_in
_the_failure` (`baseline.rs:1771`) and `a_truncated_image_is_reported_rather_than_decoded_as_
garbage` (`baseline.rs:1790`). Neither builds a well-formed-but-hostile IHDR, and nothing tests
the inflate path with an adversarial stream. There is no fuzz target in the workspace.

**What a panic or an OOM costs.** `run_scenario` calls `stage.teardown()` (`scenario.rs:350`) and
`report::write_report` (`scenario.rs:378`) on the normal return path; there is no `catch_unwind`
anywhere in `fleet-harness` (`grep -rn "catch_unwind" crates/fleet-harness/src` → nothing but my
temporary test). An unwind out of `expand` therefore skips the ordered teardown and writes no
`report.md`. `run.jsonl` survives because it is appended per line.

---

### C17 (F1-5) — verdict: SURVIVES NARROWED

- **Confidence**: high

- **Refutation attempt**
  1. *Is there a dimension cap?* No. `parse_header` rejects only `width == 0 || height == 0`
     (`baseline.rs:701-706`). Colour type 6 at depth 16 is explicitly legal (`legal_depths` for
     `_` is `&[8, 16]`, `baseline.rs:713`).
  2. *Does the CRC gate make a hostile IHDR impractical?* No. `crc32` is a plain CRC-32 the same
     file computes for its own encoder (`baseline.rs:1234`); the repro file computes it inline.
     The hostile file is **69 bytes**.
  3. *Does something bail before line 733?* `decode` calls `zlib_decompress(&data)` before
     `expand` (`baseline.rs:669-670`), so the stream must inflate first — but a one-byte stored
     block satisfies that. No caller compares dimensions before decoding: `compare` compares
     `captured.width == stored.width` only *after* both `read_image` calls
     (`baseline.rs:384-395`).
  4. *Does `bytes_per_row` itself overflow, making the claim mis-located?* No:
     `(2^32-1) * 64 < 2^38`. The only overflowing operation is the multiply at `baseline.rs:733`,
     exactly where the finding puts it.
  5. *Is `overflow-checks` really on?* Yes — see the profile analysis above, and the repro fires.
  6. **The refutation that succeeded — the release half.** F1-5 claims that in a release build
     "the wrap can make `raw.len() == expected` accept garbage, and line 739 then hits a
     capacity-overflow abort". Both halves are false.
     - For `(bytes_per_row + 1) * height` to wrap at all with `height <= 2^32-1`, you need
       `bytes_per_row + 1 > 2^64 / (2^32-1) = 4294967297`, i.e. **`bytes_per_row >= 4 GiB`**.
     - For the hostile input, the wrapped value is `18446744009285042183`, so
       `raw.len() == expected` is *false* and a release build returns the clean named error
       "the image data is 1 bytes, not the 18446744009285042183 its header describes".
     - To tune `width`/`height` so the wrapped `expected` equals an achievable `raw.len()`, the
       attacker still needs `bytes_per_row >= 4 GiB`, so the very first scanline slice
       `&raw[start + 1..start + 1 + bytes_per_row]` (`baseline.rs:745`) is out of range and
       panics on `row == 0`. Rust's slice bounds check is not disabled by `-C
       overflow-checks=off`, so **no release build can silently accept garbage pixels**, and line
       739 is reached only in cases that panic one line later.

     Measured with `rustc -C opt-level=3 -C overflow-checks=off` on the exact expression from
     `expand`:
     ```
     bytes_per_row = 34359738360 (32.00 GiB)
     expected (wrapped) = 18446744009285042183
     capacity (wrapped) = 18446744039349813252
     overflow of (bpr+1)*h needs bpr+1 > 2^64/(2^32-1) = 4294967297
     ```

- **Reasoning (the failure path that does hold)**
  1. A baseline PNG is committed under `scenarios/baselines/virtual/<scenario>/<name>.png`.
  2. A `shot` line runs in the `virtual` lane; `Baselines::check` finds the baseline and calls
     `compare` (`baseline.rs:356`).
  3. `compare` → `read_image(baseline)` → `decode`. The signature, the chunk bounds and the two
     CRCs all pass.
  4. `parse_header` accepts `width = height = 0xFFFF_FFFF`, `depth = 16`, `colour_type = 6`.
  5. `zlib_decompress` succeeds on a one-byte stored block.
  6. `expand` computes `bytes_per_row = 34359738360`, then
     `(bytes_per_row + 1) * 4294967295` at `baseline.rs:733` — `1.47e20`, past `usize::MAX`.
  7. With `overflow-checks` on (the `dev` profile `make harness` builds), this is a panic, not
     the named `anyhow` error `read_image`'s doc comment (`baseline.rs:593`) promises. It unwinds
     past `stage.teardown()` and `report::write_report`, so the run writes no report.

- **Evidence**
  ```rust
  // crates/fleet-harness/src/baseline.rs:732
  let bytes_per_row = (header.width as usize * bits_per_pixel).div_ceil(8);
  let expected = (bytes_per_row + 1) * header.height as usize;
  ```
  Temporary test (reverted) feeding a hand-built 69-byte PNG with IHDR
  `ffffffff ffffffff 10 06 00 00 00` and a correct CRC into `decode`:
  ```
  running 4 tests
  test baseline::adversarial_tmp::c17_hostile_ihdr_overflows ... C17: png is 69 bytes
  thread '...c17_hostile_ihdr_overflows' panicked at crates/fleet-harness/src/baseline.rs:733:20:
  attempt to multiply with overflow
  ```
  Reproduced twice (default target dir, and a clean scratch target dir), rustc 1.97.1.

- **Severity judgement**: **P3** (the original called it P2).
  - It is *not* reachable by corruption: a bit-flipped baseline dies on the IHDR CRC
    (`baseline.rs:653`). Only a deliberately hand-built file reaches line 733, so this is a
    hostile-input issue, not a robustness-against-corrupt-files issue.
  - The hostile input can only arrive as a committed baseline in a branch, and running
    `make harness` on a branch already executes that branch's code and build scripts — the
    trust boundary is not crossed. Today the exposure is nil in practice: `scenarios/baselines/`
    holds only `README.md` and `virtual/.gitkeep` (no baseline is committed yet), and the repo
    has no `.github/workflows` at all, so nothing decodes PR-supplied pixels automatically.
  - No memory unsafety: the worst outcome is a crashed harness run and a missing `report.md`.
  - What genuinely justifies fixing it is the contract, not the threat: every other malformed
    shape in this decoder is a named `anyhow` error, `read_image` documents that it "names the
    file in every failure", and CLAUDE.md's non-negotiables ban panicking paths in production
    code. The fix is two `checked_mul`s. A reviewer arguing P2 on contract grounds is not wrong;
    I call P3 because the blast radius is one developer's test run.

- **The exact reduced claim that holds**
  > `expand` computes `expected` at `baseline.rs:733` with unchecked `usize` arithmetic on
  > IHDR-supplied dimensions that `parse_header` bounds only away from zero. A 69-byte PNG with
  > `width = height = 0xFFFF_FFFF`, colour type 6, depth 16 makes that multiply overflow, and
  > because `make harness` builds through the `dev` profile (root `Cargo.toml` sets no
  > `overflow-checks`, `Makefile:10` sets `RELEASE ?= 0`), the harness panics instead of
  > returning the named error `read_image` documents — skipping `stage.teardown()` and
  > `report::write_report`. The `Vec::with_capacity` at `baseline.rs:739` is the same unchecked
  > shape and should be fixed with it.
  >
  > Dropped from the original: the release-build half. With `overflow-checks` off the wrap
  > cannot make `raw.len() == expected` accept garbage, because any wrap requires
  > `bytes_per_row >= 4 GiB` and the first scanline slice at `baseline.rs:745` then panics on
  > `row == 0`; for the stated input a release build returns a clean named error.
  > Also dropped: `update_baseline` (`baseline.rs:508`) is not an attacker-reachable decode.

---

### C18 (F1-6) — verdict: SURVIVES NARROWED

- **Confidence**: high

- **Refutation attempt**
  1. *Is there an output cap anywhere in the inflate path?* No. `inflate` (`baseline.rs:1053`)
     starts `let mut output = Vec::new();` and loops over blocks; `compressed_block`
     (`baseline.rs:1097`) pushes literals and copies back-references with no reference to any
     expected size; `stored_block` likewise. Nothing is threaded in from `Header`.
  2. *Does the size check run early enough?* No. `decode` does
     `let raw = zlib_decompress(&data)?;` then `expand(header, &raw, …)` (`baseline.rs:669-670`),
     so `expand`'s `ensure!` at `baseline.rs:734` sees a fully materialised `Vec`. Confirmed by
     repro: a 162 KB PNG whose IHDR says `1×1` inflates to 25,800,001 bytes and only then errors
     with "the image data is 25800001 bytes, not the 2 its header describes".
  3. *Does the Adler-32 trailer gate it?* No — `adler32(&output)` is computed over the finished
     output (`baseline.rs:910`), after the allocation. It is a cost, not a bound. (The attacker
     can compute it offline anyway; the repro does.)
  4. *Does the chunk CRC gate it?* No — CRC-32 covers the *compressed* IDAT bytes.
  5. *Is there a hidden bound in the Huffman tables?* No, but they are themselves bounded
     (`lengths` is at most 320 entries), so that is not an alternative amplifier.
  6. *Is the amplification per-block-bounded?* `stored_block` gives 1:1 at best, so multi-block
     stuffing does not help; each compressed block pays its own header.
  7. **The refutation that partly succeeded — the magnitude.** "A 1 MB IDAT expands to hundreds
     of gigabytes" is wrong by about three orders of magnitude. DEFLATE's maximum expansion is
     **1032:1** (258 output bytes for a minimum 2-bit match: a 1-bit length code plus a 1-bit
     distance code). I confirmed this decoder accepts the 1-bit distance code that the ceiling
     needs — `Huffman::build(&[1,0,0,…])` succeeds (`left = 2 - 1 = 1 >= 0`,
     `baseline.rs:987-994`) — so the ceiling is reachable here and is a ceiling. A 1 MB IDAT
     therefore tops out near **1 GB**, not hundreds of GB; hundreds of GB needs a several-hundred-MB
     committed PNG, which a reviewer would notice.
  8. *Is `update_baseline` a second entry point, as claimed?* No — see the shared groundwork; it
     decodes the fresh capture only.

- **Reasoning (the failure path that does hold)**
  1. A committed baseline PNG carries an IDAT whose deflate stream is nothing but maximal-length
     back-references.
  2. `compare` → `read_image(baseline)` → `decode` → `zlib_decompress` → `inflate`.
  3. `output` grows to up to 1032× the IDAT byte count, with `Vec` doubling putting peak RSS at
     roughly 1.5-2× that, before any size check exists to stop it.
  4. `expand`'s `ensure!` at `baseline.rs:734` would reject the result — if the process is still
     alive. A 5 MB baseline (unremarkable for a 1920×1080 screenshot) yields ~5 GB; a 20 MB one
     yields ~20 GB and OOM-kills the runner.
  5. SIGKILL runs no `Drop` and no unwinding, so `stage.teardown()` and `report::write_report`
     never run: no `report.md`, and the children are left to the detached watchdog.

- **Evidence**
  ```rust
  // crates/fleet-harness/src/baseline.rs:1119 (compressed_block)
  anyhow::ensure!(distance <= output.len(), "a deflate back-reference points before …");
  let start = output.len() - distance;
  for step in 0..length {
      let byte = output[start + step];
      output.push(byte);          // no bound on output.len()
  }
  ```
  Temporary tests (reverted). A *fixed*-Huffman bomb — one literal, then N `(length 285 = 258,
  distance 0 = 1)` pairs at 13 bits each — is enough to show the absence of a bound:
  ```
  C18: 1628 deflate bytes -> 258001 output bytes (ratio 158.5:1)
  C18: 16253 deflate bytes -> 2580001 output bytes (ratio 158.7:1)
  C18: 162503 deflate bytes -> 25800001 output bytes (ratio 158.8:1)
  ```
  And through a real PNG file, proving the ordering (inflate first, size check after):
  ```
  C18: png file is 162566 bytes
  C18: decode error after inflating 25800001 bytes:
       the image data is 25800001 bytes, not the 2 its header describes
  ```
  The tests were sized to stay in tens of MB deliberately; the growth is linear in N with no
  check anywhere, and a dynamic-Huffman block raises the constant from 158:1 to the 1032:1
  ceiling.

- **Severity judgement**: **P3** (the original called it P2).
  - Same single entry point and the same weak threat model as C17: a deliberately crafted
    committed baseline, decoded only by a developer running `make harness` in the `virtual` lane
    on a branch whose code they are already running. No baselines are committed today and there
    is no CI.
  - The corrupt-file case is not reachable: random corruption dies on the IDAT CRC, and an
    honest oversized baseline is bounded by its own file size times 1032.
  - The consequence is a dead runner and a missing `report.md`, not unsafety or a wrong verdict.
  - It is still worth fixing for the same reason as C17 — the module is a hand-rolled parser and
    every other malformed shape in it is a named error — and the fix is one `ensure!` against a
    ceiling threaded in from `Header`, which `zlib_decompress` already has at its only call site
    (`baseline.rs:670`). Fix it in the same commit as C17, since the safe `expected` computation
    is the ceiling.

- **The exact reduced claim that holds**
  > `inflate` has no output bound: `output` grows in `compressed_block` and `stored_block` with
  > no reference to the expected raw size, and the only size check (`baseline.rs:734`) runs after
  > `zlib_decompress` has returned a complete `Vec`. A crafted baseline PNG therefore forces an
  > allocation of up to 1032× its IDAT bytes — DEFLATE's maximum expansion, and a ceiling this
  > decoder can actually reach — before anything rejects it; an OOM kill loses the ordered
  > teardown and `report.md`.
  >
  > Dropped from the original: "a 1 MB IDAT expands to hundreds of gigabytes" (the real figure is
  > about 1 GB; hundreds of GB needs a several-hundred-MB PNG), and the claim that
  > `update_baseline`'s validating decode (`baseline.rs:508`) is a second reachable entry point —
  > it decodes the fresh capture, never the stored baseline.

---

## Notes for the fix

Both defects are one change: compute the raw size once, safely, and use it twice.

```
let expected = u64::from(header.height)
    .checked_mul(bytes_per_row as u64 + 1)
    .and_then(|size| usize::try_from(size).ok())
    .with_context(|| format!("the image header describes {}×{} pixels, …", …))?;
```

Thread `expected` into `zlib_decompress` → `inflate` → `compressed_block`/`stored_block` and
`ensure!(output.len() <= expected, …)` after each push site (checking once per block is not
enough — one block can be the whole bomb). Guard the `Vec::with_capacity` product at
`baseline.rs:739` the same way, or drop the hint. A regression test belongs beside the two
existing malformed-input tests (`baseline.rs:1771`, `baseline.rs:1790`): a hand-built hostile
IHDR must produce a named error, and a small bomb must be rejected without allocating.

## Method note

The full-decoder release-build repro (`RUSTFLAGS="-C overflow-checks=off"`) did not run: that
invocation surfaced two unrelated compile errors in `report.rs`'s test module (E0716 at
`report.rs:1622`, plus an E0599) that do not appear without the flag, which I did not chase. The
release analysis above is therefore arithmetic — the exact wrapped values were measured with a
standalone `rustc -C overflow-checks=off` program running the same expression — plus the
observation that slice bounds checks are unaffected by that flag. The `dev`-profile claim, which
is the one `make harness` depends on, was reproduced directly against the real decoder.

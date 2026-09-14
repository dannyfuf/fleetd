# Screenshot baselines

Committed screenshots that a harness run compares its own captures against. They are the only
part of the harness that a code change can make fail without anybody writing an assertion, which
is exactly what makes them worth keeping and exactly what makes them easy to ruin.

## What they are for

Structured dumps are the oracle: a scenario asserts on the `UiSnapshot`, and that is what says
whether the app behaved. Pixels are evidence — they catch the regressions an assertion never
names, a token that moved, a panel that lost its padding, an icon that stopped rendering. A
scenario whose only check is a screenshot is not allowed (`docs/TESTING-HARNESS.md`).

## Layout

```text
scenarios/baselines/<lane>/<scenario>/<NNN>-<name>.png
scenarios/baselines/virtual/hub/help/003-help.png     # for scenarios/hub/help.txt, line 3
```

`<scenario>` is the scenario file's path under `scenarios/` without its extension, so the
baselines mirror the corpus and two scenarios that share a file name never share a directory.
The file name is the run's own shot name, copied verbatim: `<line number>-<shot name>.png`.

Only `virtual/` exists. The `headless` lane has no pixels at all, and `attach` captures the
developer's own session — a baseline recorded there would carry their wallpaper, their cursor and
their compositor's decorations, and no other machine could reproduce it. `fleet-harness` compares
and records in the `virtual` lane and nowhere else.

## Comparison policy

Two budgets, both spent per shot (`crates/fleet-harness/src/baseline.rs`):

- **a per-channel threshold** — a pixel counts as changed only when one of its channels moves by
  more than 8. GPU rasterisation and font fallback move a glyph's antialiasing by less than that.
- **a differing-pixel budget** — the shot fails when more than 0.2% of the image counts as
  changed. On a 1920×1080 output that is about 4,000 pixels: a caret, a focus ring, one relaid-out
  label. A recoloured surface is an order of magnitude more and always fails.

A failing comparison writes `shots/<NNN>-<name>-diff.png` into the run directory: the baseline,
washed out, with every differing pixel in magenta. The run directory also holds the capture, so
the two can be opened side by side.

Two things are deliberately *not* failures:

- **No baseline yet.** A new scenario runs green and the report says where its baseline would go.
- **A missing baseline in another lane.** Nothing is compared outside `virtual`.

A window-size change *is* an error rather than a difference: nothing useful can be said about
which pixels moved when the window did, and the answer is always to re-record.

## Updating

```sh
cargo run -p fleet-harness -- run scenarios/hub/help.txt --update-baselines
cargo run -p fleet-harness -- run scenarios/ --update-baselines      # the whole corpus
```

`--update-baselines` replaces every baseline the run captured, and the run reports which files
actually changed so a re-record of the whole corpus says how much of it there is to read.

**Review the diff.** A baseline is committed so that a change to it is visible in a pull request.
`git diff` will not show you a PNG, so open the images: a baseline update should be a change you
can name in the commit message ("the surface tone moved with the token"), never a line of noise
you re-recorded to get a run green.

## Keeping this reviewable

Take few, deliberate screenshots — one per scenario, at the moment that matters, never one per
step. A theme or font change rewrites every baseline at once, and a hundred-image update is one
nobody reviews. If a scenario needs more than one shot to be worth reading, that is usually two
scenarios.

## Nothing is recorded yet

`virtual/` is empty, and every `shot` in the corpus reports `baseline: none yet …` in its run
report. That is not an oversight: §9.8 of `docs/TESTING-HARNESS.md` forbids recording a baseline
nobody has looked at, and on the machine this corpus was last run on nobody can look at one — the
session paints a lock surface over every compositor output, so the lane's own guard refuses each
capture rather than filing a photograph of it here. `docs/TESTING-HARNESS.md` §11 is where that
is tracked. The first person to run `make harness HARNESS_ARGS=--update-baselines` on an unlocked
session should open all 38 images before committing them.

#!/bin/zsh
# make-repo.sh <directory> [dirty|clean|conflict|merging|reflog|tree|ts]
#
# Builds the throwaway repository the drive scripts assume, under <directory>:
#
#   <directory>/origin.git   a bare "origin"
#   <directory>/work         the work tree, `main` checked out and pushed
#
#   commits   c1 add alpha (alpha.txt, 20 lines) · c2 add beta · c3 change alpha
#             (line2) · c4 add gamma            — tag v1.0 on c4
#   branches  main (upstream origin/main) and feature (f1 add feature,
#             f2 extend feature), whose commits are newer than main's
#   stash     one entry, "wip beta"
#   dirt      alpha.txt modified on lines 4 and 17 (two hunks), gamma.txt
#             staged, untracked.txt untracked
#
# `clean`    resets the worktree, so the interactive-rebase flows do not have to
#            reason about the autostash.
# `conflict` additionally puts a commit on main that touches feature.txt, so
#            rebasing feature onto main conflicts. Implies `clean`.
# `reflog`   additionally leaves `commit: z1 lost commit` as reflog row 2, by
#            committing and then resetting. Implies `clean`.
# `merging`  additionally stops the repository mid-merge with exactly one
#            conflicted file, conflict.txt (ours vs theirs). Implies `clean`.
# `tree`     additionally commits nested directories and dirties them, so the
#            Files pane has something to draw a tree from. Implies `dirty`, and
#            the pane then renders, in order:
#
#              1  ▼ pkg                 2    M  three.txt
#              3  ▼ src/app/deep        4    ?? new.txt      5   M one.txt
#              6   M alpha.txt          7  M  gamma.txt      8  ?? untracked.txt
#
#            (1-based, like every other row number in this directory.)
# `ts`       additionally commits and then edits TypeScript sources, so the diff
#            view has syntax highlighting and word-level marks to draw. Implies
#            `dirty`, and the Files pane then renders, in order:
#
#              1   M alpha.txt    2   M demo.ts    3   M demo.tsx
#              4 M  gamma.txt    5  ?? long.ts   6  ?? untracked.txt
set -eu
ROOT=$1
MODE=${2:-dirty}
rm -rf "$ROOT"; mkdir -p "$ROOT"
export GIT_AUTHOR_NAME=Tester GIT_AUTHOR_EMAIL=tester@example.com
export GIT_COMMITTER_NAME=Tester GIT_COMMITTER_EMAIL=tester@example.com
git init --bare -q "$ROOT/origin.git"
git init -q -b main "$ROOT/work"
cd "$ROOT/work"
git remote add origin "$ROOT/origin.git"
# Local config, so the app's own `git` calls do not pick up a signing key.
git config user.name Tester
git config user.email tester@example.com
git config commit.gpgsign false

for i in $(seq 1 20); do echo "line$i"; done > alpha.txt
git add alpha.txt && git commit -qm "c1 add alpha"
printf 'beta one\n' > beta.txt
git add beta.txt && git commit -qm "c2 add beta"
sed -i '' 's/^line2$/line2 CHANGED/' alpha.txt
git add alpha.txt && git commit -qm "c3 change alpha"
printf 'gamma\n' > gamma.txt
git add gamma.txt && git commit -qm "c4 add gamma"
git push -q -u origin main
git tag -f v1.0 >/dev/null

git checkout -q -b feature
printf 'feature one\n' > feature.txt
git add feature.txt && git commit -qm "f1 add feature"
printf 'feature one\nfeature two\n' > feature.txt
git add feature.txt && git commit -qm "f2 extend feature"
git checkout -q main

if [[ "$MODE" == ts ]]; then
  cat > demo.ts <<'TS'
// Order helpers for the storefront.
import { Money } from "./money";

export interface CartItem {
  sku: string;
  quantity: number;
  price: Money;
}

export const TAX_RATE = 0.2;

export function subtotal(items: CartItem[]): Money {
  return items.reduce((sum, item) => sum + item.price * item.quantity, 0);
}

export function total(items: CartItem[]): Money {
  return subtotal(items) * (1 + TAX_RATE);
}
TS
  cat > demo.tsx <<'TSX'
import React from "react";

export function Badge({ label, tone }: { label: string; tone: string }) {
  return <span className={`badge badge-${tone}`}>{label}</span>;
}
TSX
  git add demo.ts demo.tsx && git commit -qm "s1 add typescript sources"
fi

if [[ "$MODE" == tree ]]; then
  mkdir -p src/app/deep pkg
  printf 'one\n' > src/app/deep/one.txt
  printf 'two\n' > src/app/deep/two.txt
  printf 'three\n' > pkg/three.txt
  git add src pkg && git commit -qm "t1 add nested files"
fi

printf 'stashed content\n' >> beta.txt
git stash push -q -m "wip beta"

sed -i '' -e 's/^line4$/line4 DIRTY/' -e 's/^line17$/line17 DIRTY/' alpha.txt
printf 'staged change\n' > gamma.txt
git add gamma.txt
printf 'brand new\n' > untracked.txt

if [[ "$MODE" == ts ]]; then
  # Single-word edits, so the intra-line pass has something to mark rather than
  # tinting whole lines.
  sed -i '' -e 's/  sku: string;/  sku: SkuCode;/' \
            -e 's/export const TAX_RATE = 0.2;/export const TAX_RATE = 0.21;/' \
            -e 's/(1 + TAX_RATE)/(1 + TAX_RATE + FEE)/' demo.ts
  sed -i '' -e 's/tone: string }/tone: Tone }/' \
            -e 's/badge badge-/chip chip-/' demo.tsx
  # A long untracked file, so the wheel has somewhere to scroll to, ending in three
  # lines wide enough that `H` / `L` have somewhere to pan.
  { print '// A hundred lines of nothing in particular.'
    for i in $(seq 1 120); do print "export const value$i = $i;"; done
    for i in 1 2 3; do
      print -n "export const wide$i = ["
      for j in $(seq 1 24); do print -n "\"segment-$j-of-a-deliberately-very-long-line\", "; done
      print "];"
    done } > long.ts
fi

if [[ "$MODE" == tree ]]; then
  printf 'one changed\n' > src/app/deep/one.txt
  printf 'three changed\n' > pkg/three.txt
  git add pkg/three.txt
  printf 'nested new\n' > src/app/deep/new.txt
fi

if [[ "$MODE" != dirty && "$MODE" != tree && "$MODE" != ts ]]; then
  git reset -q --hard HEAD
  rm -f untracked.txt
fi
if [[ "$MODE" == conflict ]]; then
  # A second of separation, so `for-each-ref --sort=-committerdate` really puts
  # main above feature: the drive scripts select main with `<`.
  sleep 1.2
  printf 'main version\n' > feature.txt
  git add feature.txt && git commit -qm "m1 main touches feature.txt"
  git checkout -q feature
fi
if [[ "$MODE" == merging ]]; then
  printf 'base\n' > conflict.txt
  git add conflict.txt && git commit -qm "k0 base conflict file"
  git checkout -q -b other
  printf 'theirs\n' > conflict.txt
  git add conflict.txt && git commit -qm "k1 theirs"
  git checkout -q main
  printf 'ours\n' > conflict.txt
  git add conflict.txt && git commit -qm "k2 ours"
  git merge other >/dev/null 2>&1 || true
fi
if [[ "$MODE" == reflog ]]; then
  printf 'lost\n' > lost.txt
  git add lost.txt && git commit -qm "z1 lost commit"
  git reset -q --hard HEAD~1
fi
echo "$ROOT/work"

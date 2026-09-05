#!/bin/zsh
# run-suite.sh [name-filter ...]
#
# Runs every `drive/*.txt` script against a freshly built throwaway repository
# and reports one line per script. With arguments, only the scripts whose name
# contains one of them run.
#
# Each script has one manifest row below, giving
#
#   mode    which `make-repo.sh` tree it needs
#   prep    shell run in the work tree before the app starts
#   assert  shell run in the work tree afterwards; a non-zero exit fails the row
#
# A row also fails when the driver did not reach `done quit`, when it logged an
# `unhandled key` (a binding the script expected is missing), or when the app
# printed a panic. `$SCRATCH` chooses where the throwaway repos and logs land.
set -u
HERE=${0:a:h}
ROOT=$(git -C "$HERE" rev-parse --show-toplevel)
BIN=${FLEET_LAZYGIT_BIN:-$ROOT/target/release/fleet-lazygit}
SCRATCH=${SCRATCH:-${TMPDIR:-/tmp}/fleet-lazygit-suite}
[[ -x $BIN ]] || { print -u2 "no binary at $BIN — cargo build --release -p fleet-lazygit"; exit 1 }
rm -rf "$SCRATCH"; mkdir -p "$SCRATCH"

# ---------------------------------------------------------------- the manifest
mode() {
  case $1 in
    branches-delete|branches-delete-force|branches-enter|branches-merge) print clean ;;
    branches-fetch-pull|branches-push|branches-subcommits-cherry-pick) print clean ;;
    branches-rebase) print clean ;;
    branches-rebase-conflict-*) print conflict ;;
    branches-rebase-dirty) print dirty ;;
    branches-rename|branches-upstream-*|error-slot) print dirty ;;
    diff-*) print ts ;;
    files-tree-*) print tree ;;
    commits-copy-paste-*) print clean ;;
    commits-*) print clean ;;
    conflict-take-*) print merging ;;
    quit-during-operation) print clean ;;
    reflog-*) print reflog ;;
    remotes-*) print dirty ;;
    menu-letter-reset) print clean ;;
    *) print dirty ;;
  esac
}

prep() {
  case $1 in
    branches-delete)                print 'git branch -q merged-b main' ;;
    branches-fetch-pull)            print "'$HERE/make-origin-ahead.sh' \"\$REPO_ROOT\"" ;;
    branches-push)                  print 'git commit -q --allow-empty -m "local ahead"' ;;
    branches-rebase|branches-rebase-dirty) print 'git checkout -q feature' ;;
    branches-subcommits-cherry-pick) print '' ;;
    commits-copy-paste-*)           print 'git checkout -q feature' ;;
    quit-during-operation)          print 'GIT_SEQUENCE_EDITOR='"'"'sed -i "" "1s/^pick/edit/"'"'"' git rebase -q -i HEAD~2 >/dev/null 2>&1; test -d .git/rebase-merge' ;;
    tags-delete)                    print 'git tag v2.0' ;;
    *)                              print '' ;;
  esac
}

# Scripts whose keys are deliberately consumed by a key listener rather than by a
# binding: `Window::dispatch_keystroke` then reports "unhandled", which is not a
# defect. Menu shortcut letters are the only such case.
tolerates_unhandled() {
  case $1 in
    menu-letter-*) return 0 ;;
    *) return 1 ;;
  esac
}

# Scripts whose app is *meant* to exit before the script ends, so `done quit` never
# lands: the success signal is the process being gone.
expects_self_exit() {
  case $1 in
    quit-during-operation) return 0 ;;
    *) return 1 ;;
  esac
}

assert() {
  case $1 in
    branches-delete)        print '! git rev-parse --verify -q merged-b && git rev-parse --verify -q feature' ;;
    branches-delete-force)  print '! git rev-parse --verify -q feature' ;;
    branches-enter)         print 'true' ;;
    branches-fetch-pull)    print 'test -f delta.txt' ;;
    branches-merge)         print 'git merge-base --is-ancestor feature main' ;;
    branches-push)          print 'test "$(git rev-parse main)" = "$(git rev-parse origin/main)"' ;;
    branches-rebase)        print 'git merge-base --is-ancestor main feature' ;;
    branches-rebase-dirty)  print 'git merge-base --is-ancestor main feature && grep -q "line4 DIRTY" alpha.txt' ;;
    branches-rebase-conflict-abort)    print 'test ! -d .git/rebase-merge && test "$(git rev-parse --abbrev-ref HEAD)" = feature && ! git merge-base --is-ancestor main feature' ;;
    branches-rebase-conflict-continue) print 'test ! -d .git/rebase-merge && git merge-base --is-ancestor main feature' ;;
    branches-rename)        print 'git rev-parse --verify -q feature-renamed && ! git rev-parse --verify -q feature' ;;
    branches-upstream-set)  print 'test "$(git rev-parse --abbrev-ref main@{upstream})" = origin/main' ;;
    branches-upstream-unset) print '! git rev-parse -q --verify main@{upstream}' ;;
    branches-subcommits-cherry-pick) print 'git log -1 --format=%s main | grep -q "f1 add feature"' ;;
    commits-checkout-detach) print 'test "$(git rev-parse --abbrev-ref HEAD)" = main' ;;
    commits-copy-paste-one) print 'git log --format=%s main | grep -q "f1 add feature"' ;;
    commits-copy-paste-two) print 'test "$(git log -2 --format=%s main | tr "\n" " ")" = "f2 extend feature f1 add feature "' ;;
    commits-copy-paste-conflict) print 'test ! -f .git/CHERRY_PICK_HEAD && test "$(git rev-parse --abbrev-ref HEAD)" = main' ;;
    commits-drop)           print 'test "$(git log --oneline | wc -l | tr -d " ")" = 3 && ! grep -q CHANGED alpha.txt' ;;
    commits-edit-continue)  print 'test ! -d .git/rebase-merge && test "$(git log --oneline | wc -l | tr -d " ")" = 4' ;;
    commits-edit-stop)      print 'test -d .git/rebase-merge' ;;
    commits-enter-files|commits-files-whole-patch|commits-enter-diff) print 'true' ;;
    # Diff-view rendering flows: read-only, so the success signal is reaching
    # `quit` with no panic and no unhandled key. The screenshots in the report
    # are the visual artefact.
    diff-*)                 print 'true' ;;
    commits-fixup)          print 'test "$(git log --oneline | wc -l | tr -d " ")" = 3 && ! git log --format=%s | grep -q "c3 change alpha"' ;;
    commits-move)           print 'test "$(git log -4 --format=%s | tr "\n" " ")" = "c3 change alpha c4 add gamma c2 add beta c1 add alpha "' ;;
    commits-new-branch)     print 'test "$(git rev-parse --abbrev-ref HEAD)" = topic' ;;
    commits-reset-hard)     print 'test "$(git log -1 --format=%s)" = "c2 add beta" && test ! -f gamma.txt' ;;
    commits-reset-mixed)    print 'test "$(git log -1 --format=%s)" = "c2 add beta" && test -z "$(git diff --cached --name-only)"' ;;
    commits-reset-soft)     print 'test "$(git log -1 --format=%s)" = "c2 add beta" && git diff --cached --name-only | grep -q gamma.txt' ;;
    commits-revert)         print 'git log -1 --format=%s | grep -q "^Revert" && test ! -f gamma.txt' ;;
    commits-squash)         print 'test "$(git log --oneline | wc -l | tr -d " ")" = 3' ;;
    commits-tag)            print 'git tag -l | grep -q from-commit' ;;
    conflict-take-theirs)   print 'grep -q theirs conflict.txt && ! git status --porcelain | grep -q "^UU"' ;;
    conflict-take-both)     print 'grep -q ours conflict.txt && grep -q theirs conflict.txt' ;;
    error-slot)             print 'test "$(git rev-parse --abbrev-ref HEAD)" = feature' ;;
    files-amend)            print 'test "$(git log --oneline | wc -l | tr -d " ")" = 4 && git show HEAD:gamma.txt | grep -q "staged change"' ;;
    files-discard-tracked)  print '! grep -q DIRTY alpha.txt' ;;
    files-discard-untracked) print 'test ! -f untracked.txt' ;;
    files-selection-retained) print 'git diff --cached --name-only | grep -q gamma.txt && ! git diff --cached --name-only | grep -q alpha.txt' ;;
    files-tree-render)      print 'true' ;;
    files-tree-stage-directory) print 'git diff --cached --name-only | grep -q "src/app/deep/one.txt" && git diff --cached --name-only | grep -q "src/app/deep/new.txt"' ;;
    files-tree-collapse)    print 'git diff --cached --name-only | grep -q "src/app/deep/one.txt" && git diff --cached --name-only | grep -q "pkg/three.txt"' ;;
    files-tree-collapse-expand) print '! git diff --cached --name-only | grep -q "pkg/three.txt" && git diff --cached --name-only | grep -q gamma.txt' ;;
    files-tree-discard-directory) print 'test ! -f src/app/deep/new.txt && ! git diff --name-only | grep -q "src/app/deep/one.txt" && grep -q "line4 DIRTY" alpha.txt' ;;
    files-tree-toggle-flat) print 'git diff --cached --name-only | grep -q "src/app/deep/one.txt" && git diff --cached --name-only | grep -q "pkg/three.txt"' ;;
    files-tree-selection-retained) print '! git diff --cached --name-only | grep -q "src/app/deep" && test -f src/app/deep/new.txt && git diff --cached --name-only | grep -q gamma.txt' ;;
    files-stage-all)        print 'test "$(git diff --cached --name-only | wc -l | tr -d " ")" = 3' ;;
    files-stage-all-unstage-all) print 'test -z "$(git diff --cached --name-only)"' ;;
    files-stash-staged)     print 'test "$(git stash list | wc -l | tr -d " ")" = 2 && ! grep -q "staged change" gamma.txt' ;;
    files-stash-untracked)  print 'test "$(git stash list | wc -l | tr -d " ")" = 2 && test ! -f untracked.txt' ;;
    help-scroll-clamp)      print 'true' ;;
    menu-letter-reset)      print 'test "$(git log -1 --format=%s)" = "c2 add beta" && git diff --cached --name-only | grep -q gamma.txt' ;;
    menu-letter-stash)      print 'test "$(git stash list | wc -l | tr -d " ")" = 2 && test ! -f untracked.txt' ;;
    menu-filter-then-letter) print 'test "$(git stash list | wc -l | tr -d " ")" = 2 && test ! -f untracked.txt' ;;
    quit-during-operation)  print 'test -d .git/rebase-merge' ;;
    reflog-checkout)        print 'test -f lost.txt && test "$(git rev-parse --abbrev-ref HEAD)" = HEAD' ;;
    reflog-cherry-pick)     print 'test -f lost.txt && test "$(git rev-parse --abbrev-ref HEAD)" = main' ;;
    remotes-checkout)       print 'git rev-parse -q --verify main@{upstream} >/dev/null' ;;
    remotes-enter-subcommits) print 'true' ;;
    staging-discard-hunk)   print '! grep -q "line4 DIRTY" alpha.txt && grep -q "line17 DIRTY" alpha.txt' ;;
    staging-side-flips-when-empty) print 'git diff --name-only | grep -q alpha.txt' ;;
    staging-stage-hunk)     print 'git diff --cached | grep -q "line4 DIRTY" && ! git diff --cached | grep -q "line17 DIRTY"' ;;
    staging-unstage-hunk)   print 'git diff --cached | grep -q "line17 DIRTY" && git diff | grep -q "line4 DIRTY"' ;;
    staging-unstage-line)   print 'git diff --cached | grep -q "line4 DIRTY"' ;;
    stash-apply)            print 'grep -q "stashed content" beta.txt && test "$(git stash list | wc -l | tr -d " ")" = 1' ;;
    stash-branch)           print 'test "$(git rev-parse --abbrev-ref HEAD)" = from-stash' ;;
    stash-drop)             print 'test -z "$(git stash list)"' ;;
    stash-enter-diff)       print 'true' ;;
    stash-pop)              print 'grep -q "stashed content" beta.txt && test -z "$(git stash list)"' ;;
    tags-checkout)          print 'test "$(git rev-parse --abbrev-ref HEAD)" = HEAD' ;;
    tags-create)            print 'git tag -l | grep -q "^v2.0$"' ;;
    tags-delete)            print '! git tag -l | grep -q "^v2.0$" && git tag -l | grep -q "^v1.0$"' ;;
    *)                      print 'true' ;;
  esac
}

# ---------------------------------------------------------------- the loop
pass=0; fail=0; failed=()
for script in "$HERE"/*.txt; do
  name=${script:t:r}
  if (( $# > 0 )); then
    keep=0
    for filter in "$@"; do [[ $name == *$filter* ]] && keep=1; done
    (( keep )) || continue
  fi
  work=$SCRATCH/$name
  "$HERE/make-repo.sh" "$work" "$(mode $name)" >/dev/null || { print "FAIL $name (make-repo)"; (( fail++ )); failed+=$name; continue }
  export REPO_ROOT=$work
  p=$(prep $name)
  if [[ -n $p ]]; then
    ( cd "$work/work" && eval "$p" ) >/dev/null 2>&1 \
      || { print "FAIL $name (prep)"; (( fail++ )); failed+=$name; continue }
  fi
  drive=$work/drive.txt; log=$drive.log
  : > "$drive"
  RUST_LOG=warn FLEET_LAZYGIT_DRIVE="$drive" "$BIN" "$work/work" > "$work/app.log" 2>&1 &
  app=$!
  for i in $(seq 1 100); do
    [[ -f $log ]] && grep -q ' start ' "$log" && break
    sleep 0.1
  done
  cat "$script" >> "$drive"
  for i in $(seq 1 900); do
    grep -q 'done quit' "$log" 2>/dev/null && break
    kill -0 $app 2>/dev/null || break
    sleep 0.1
  done
  sleep 0.4
  alive=0; kill -0 $app 2>/dev/null && alive=1
  kill $app 2>/dev/null; wait $app 2>/dev/null
  why=""
  if expects_self_exit $name; then
    (( alive )) && why="the app did not quit by itself"
  else
    grep -q 'done quit' "$log" 2>/dev/null || why="never reached quit"
  fi
  if ! tolerates_unhandled $name; then
    grep -q 'unhandled key' "$log" 2>/dev/null && why="${why:+$why; }unhandled key: $(grep -m1 'unhandled key' "$log")"
  fi
  grep -qi 'panicked' "$work/app.log" 2>/dev/null && why="${why:+$why; }panic"
  a=$(assert $name)
  ( cd "$work/work" && eval "$a" ) >/dev/null 2>&1 || why="${why:+$why; }assertion failed: $a"
  if [[ -z $why ]]; then
    print "ok   $name"; (( pass++ ))
  else
    print "FAIL $name — $why"; (( fail++ )); failed+=$name
  fi
done
print ""
print "$pass passed, $fail failed"
(( fail == 0 )) || { print "failed: ${failed[*]}"; exit 1 }

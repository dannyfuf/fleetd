#!/bin/zsh
# make-origin-ahead.sh <directory>
# Puts one commit on the bare origin of a make-repo.sh tree that the work tree
# has not seen, so `f` has something to fetch and `p` something to pull.
set -eu
ROOT=$1
TMP=$(mktemp -d)
git clone -q "$ROOT/origin.git" "$TMP/clone"
cd "$TMP/clone"
git config user.name Other; git config user.email other@example.com
git config commit.gpgsign false
printf 'delta from origin\n' > delta.txt
git add delta.txt && git commit -qm "o1 origin adds delta"
git push -q origin HEAD:main
rm -rf "$TMP"

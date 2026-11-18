#!/usr/bin/env bash
set -euo pipefail
PR_NUMBER="$1"

gh pr checkout "$PR_NUMBER" --force

MERGEABLE=$(gh pr view "$PR_NUMBER" --json mergeable -q .mergeable)
if [ "$MERGEABLE" = "CONFLICTING" ]; then
  echo "ERROR: PR #$PR_NUMBER has merge conflicts with main (rebase needed)" >&2
  exit 1
fi

just check
just update-nix-hash
just check-nix

if ! git diff --quiet; then
  git add -A
  git commit -m "chore(deps): update cargoHash"
  git push
fi

for i in $(seq 1 6); do
  gh pr checks "$PR_NUMBER" >/dev/null 2>&1 && break
  sleep 5
done

gh pr merge "$PR_NUMBER" --auto --squash --delete-branch

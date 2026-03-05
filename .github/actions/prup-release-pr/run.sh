#!/usr/bin/env bash
set -euo pipefail

base_branch="${INPUT_BASE_BRANCH:-main}"
head_branch="${INPUT_HEAD_BRANCH:-}"
if [[ -z "$head_branch" ]]; then
  head_branch="codex/prup-release-${GITHUB_RUN_ID:-local}"
fi

if [[ "$head_branch" != codex/* ]]; then
  echo "head branch must start with codex/: $head_branch" >&2
  exit 1
fi

plan_json="$(mktemp)"

cargo run --locked -p prup -- doctor

git checkout -B "$head_branch"
cargo run --locked -p prup -- plan --format json --output "$plan_json"

if jq -e '.crate_updates | length == 0' "$plan_json" >/dev/null; then
  echo "No release updates detected."
  exit 0
fi

cargo run --locked -p prup -- apply --from-plan "$plan_json"

git add -A
if git diff --cached --quiet; then
  echo "No staged changes after apply."
  exit 0
fi

git config --global user.name "prup[bot]"
git config --global user.email "prup[bot]@users.noreply.github.com"

git commit \
  -m "ci: prepare release with prup" \
  -m "Co-authored-by: Codex <noreply@openai.com>"

git push -u origin "$head_branch"

existing_pr_number="$(gh pr list --head "$head_branch" --base "$base_branch" --state open --json number --jq '.[0].number // empty')"

if [[ -n "$existing_pr_number" ]]; then
  gh pr edit "$existing_pr_number" --title "$INPUT_PR_TITLE" --body "$INPUT_PR_BODY"
  echo "Updated existing PR #$existing_pr_number"
else
  gh pr create \
    --base "$base_branch" \
    --head "$head_branch" \
    --title "$INPUT_PR_TITLE" \
    --body "$INPUT_PR_BODY"
  echo "Created new release PR from $head_branch"
fi

#!/usr/bin/env bash
set -euo pipefail

base_branch="${INPUT_BASE_BRANCH:-main}"

targets_json="$(mktemp)"

cargo run --locked -p prup -- doctor
cargo run --locked -p prup -- release-pr-targets --format json --output "$targets_json"

if jq -e '.targets | length == 0' "$targets_json" >/dev/null; then
  echo "No release updates detected."
  exit 0
fi

git config --global user.name "prup[bot]"
git config --global user.email "prup[bot]@users.noreply.github.com"

while IFS= read -r row; do
  line_id="$(jq -r '.line_id' <<<"$row")"
  head_branch="$(jq -r '.branch' <<<"$row")"
  title="$(jq -r '.title' <<<"$row")"
  body_file="$(mktemp)"
  plan_json="$(mktemp)"

  if [[ "$head_branch" != codex/* ]]; then
    echo "head branch must start with codex/: $head_branch" >&2
    exit 1
  fi

  jq -r '.body' <<<"$row" >"$body_file"

  git checkout -B "$head_branch" "origin/$base_branch"
  cargo run --locked -p prup -- plan --line "$line_id" --format json --output "$plan_json"

  if jq -e '.crate_updates | length == 0' "$plan_json" >/dev/null; then
    echo "No release updates detected for $line_id."
    continue
  fi

  cargo run --locked -p prup -- apply --from-plan "$plan_json"
  cargo check --workspace

  git add -A
  if git diff --cached --quiet; then
    echo "No staged changes after apply for $line_id."
    continue
  fi

  git commit \
    -m "$title" \
    -m "Co-authored-by: Codex <noreply@openai.com>"

  git push --force-with-lease -u origin "$head_branch"

  existing_pr_number="$(gh pr list --head "$head_branch" --base "$base_branch" --state open --json number --jq '.[0].number // empty')"

  label_args=()
  while IFS= read -r label; do
    [[ -z "$label" ]] && continue
    label_args+=(--label "$label")
  done < <(jq -r '.labels[]?' <<<"$row")

  if [[ -n "$existing_pr_number" ]]; then
    gh pr edit "$existing_pr_number" --title "$title" --body-file "$body_file"

    if ((${#label_args[@]} > 0)); then
      edit_label_args=()
      for ((i = 0; i < ${#label_args[@]}; i += 2)); do
        edit_label_args+=(--add-label "${label_args[i + 1]}")
      done
      gh pr edit "$existing_pr_number" "${edit_label_args[@]}"
    fi

    echo "Updated existing PR #$existing_pr_number for $line_id"
  else
    gh pr create \
      --base "$base_branch" \
      --head "$head_branch" \
      --title "$title" \
      --body-file "$body_file" \
      "${label_args[@]}"
    echo "Created new release PR from $head_branch"
  fi
done < <(jq -c '.targets[]' "$targets_json")

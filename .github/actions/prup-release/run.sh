#!/usr/bin/env bash
set -euo pipefail

remote="${INPUT_REMOTE:-origin}"
targets_json="$(mktemp)"

cargo run --locked -p prup -- doctor
cargo run --locked -p prup -- release-targets --format json --output "$targets_json"

if jq -e '.targets | length == 0' "$targets_json" >/dev/null; then
  echo "No release targets."
  exit 0
fi

git config user.name "prup[bot]"
git config user.email "prup[bot]@users.noreply.github.com"

prerelease_flag="$(jq -r '.prerelease' "$targets_json")"

while IFS= read -r row; do
  crate_name="$(jq -r '.crate_name' <<<"$row")"
  version="$(jq -r '.version' <<<"$row")"
  tag="$(jq -r '.tag' <<<"$row")"
  release_name="$(jq -r '.release_name' <<<"$row")"
  github_release="$(jq -r '.github_release' <<<"$row")"
  body="$(jq -r '.body' <<<"$row")"

  if git tag --list "$tag" | grep -qx "$tag"; then
    echo "Tag already exists: $tag"
  else
    git tag -a "$tag" -m "prup release $crate_name $version"
    git push "$remote" "$tag"
    echo "Created tag: $tag"
  fi

  if [[ "$github_release" != "true" ]]; then
    continue
  fi

  if gh release view "$tag" >/dev/null 2>&1; then
    echo "GitHub Release already exists: $tag"
    continue
  fi

  if [[ "$prerelease_flag" == "true" ]]; then
    gh release create "$tag" --title "$release_name" --notes "$body" --prerelease
  else
    gh release create "$tag" --title "$release_name" --notes "$body"
  fi
  echo "Created GitHub Release: $tag"
done < <(jq -c '.targets[]' "$targets_json")

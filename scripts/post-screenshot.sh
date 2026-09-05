#!/usr/bin/env bash
#
# Publish screenshots so a pull request description can show them.
#
# GitHub renders an image in a pull request body only from a public URL that
# serves an image content type. `raw.githubusercontent.com` does. A release
# asset does not: it redirects to a signed URL that expires within the hour and
# answers `application/octet-stream`, which GitHub's image proxy refuses. So the
# images go into the repository — but onto an orphan branch that no other
# history reaches, so they never enter `main` and can be deleted whole once the
# pull request merges.
#
# Nothing is committed locally. The blob, tree, commit and ref are built through
# the GitHub API, so the working copy, the current change, and `main` are all
# left alone.
#
# Usage:
#   scripts/run-headless.sh                                    # -> /tmp/melody-shot.png
#   SLUG=jim-247 scripts/post-screenshot.sh /tmp/melody-shot.png
#
# One markdown image line is printed per file. Paste them into the description.
#
#   SLUG   branch is screenshots/$SLUG (default: a UTC timestamp). Name it after
#          the issue, so it is obvious later which pull request it belongs to.
#   REPO   owner/name (default: read from the `origin` remote)
#
# Once the pull request is merged, delete the branch:
#
#   gh api -X DELETE repos/$REPO/git/refs/heads/screenshots/$SLUG
#
# Prerequisites: `gh` authenticated with write access to the repo, and `jq`.
set -euo pipefail

[[ $# -gt 0 ]] || { echo "usage: [SLUG=name] $0 IMAGE..." >&2; exit 64; }
for image in "$@"; do
	[[ -f "$image" ]] || { echo "ERROR: no such file: $image" >&2; exit 66; }
done

origin_url() {
	jj git remote list 2>/dev/null |
		awk '$1 == "origin" { print $2; found = 1 } END { exit !found }' ||
		git remote get-url origin
}

REPO="${REPO:-$(origin_url | sed -E 's#.*github\.com[:/]##; s#\.git$##')}"
SLUG="${SLUG:-$(date -u +%Y%m%d-%H%M%S)}"
BRANCH="screenshots/${SLUG}"

# The tree holds only the files posted by this call. Earlier ones stay reachable
# through the parent commit, which keeps every URL already pasted somewhere
# working.
tree_entry() {
	local image="$1" sha
	# The base64 goes through the pipe, not through an argument: a single
	# argument is capped at 128 KiB, which a screenshot passes easily.
	sha=$(base64 -w0 "$image" | tr -d '\n' |
		jq -Rs '{content: ., encoding: "base64"}' |
		gh api "repos/${REPO}/git/blobs" --input - --jq .sha)
	jq -n --arg path "$(basename "$image")" --arg sha "$sha" \
		'{path: $path, mode: "100644", type: "blob", sha: $sha}'
}

entries=()
for image in "$@"; do
	entries+=("$(tree_entry "$image")")
done

tree=$(printf '%s\n' "${entries[@]}" | jq -s '{tree: .}' |
	gh api "repos/${REPO}/git/trees" --input - --jq .sha)

# An existing branch is extended rather than replaced, so that a URL pasted into
# an earlier description keeps resolving.
parents=$({ gh api "repos/${REPO}/git/ref/heads/${BRANCH}" 2>/dev/null || true; } |
	jq -c '[.object.sha? // empty]')

commit=$(jq -n --arg tree "$tree" --argjson parents "$parents" --arg slug "$SLUG" \
	'{message: "Screenshots for \($slug)", tree: $tree, parents: $parents}' |
	gh api "repos/${REPO}/git/commits" --input - --jq .sha)

if [[ "$parents" == "[]" ]]; then
	jq -n --arg ref "refs/heads/${BRANCH}" --arg sha "$commit" '{ref: $ref, sha: $sha}' |
		gh api "repos/${REPO}/git/refs" --input - --jq .ref >/dev/null
else
	jq -n --arg sha "$commit" '{sha: $sha}' |
		gh api -X PATCH "repos/${REPO}/git/refs/heads/${BRANCH}" --input - --jq .ref >/dev/null
fi

echo ">>> ${BRANCH} @ ${commit}" >&2
for image in "$@"; do
	name=$(basename "$image")
	printf '![%s](https://raw.githubusercontent.com/%s/%s/%s)\n' \
		"${name%.*}" "$REPO" "$commit" "$name"
done

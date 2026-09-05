#!/usr/bin/env bash
#
# Publish screenshots so a pull request description can show them.
#
# GitHub renders an image in a pull request body only from a public URL that
# serves an image content type. `raw.githubusercontent.com` does. A release
# asset does not: it redirects to a signed URL that expires within the hour and
# answers `application/octet-stream`, which GitHub's image proxy refuses. So the
# images go into the repository — but onto an orphan branch that no other
# history reaches, so they never enter `main`.
#
# Nothing is committed locally. The blob, tree, commit and ref are built through
# the GitHub API, so the working copy, the current change, and `main` are all
# left alone.
#
# Usage:
#   scripts/run-headless.sh                          # -> /tmp/melody-shot.png
#   scripts/post-screenshot.sh /tmp/melody-shot.png  # from the branch under review
#
# One markdown image line is printed per file. Paste them into the description.
#
#   SLUG   branch is screenshots/$SLUG. Defaults to the branch this runs on,
#          and that pairing is what the prune below reads. A slug that is not a
#          branch name is never cleaned up.
#   REPO   owner/name (default: read from the `origin` remote)
#
# Each run first deletes the screenshot branches whose own branch is gone. The
# repository deletes a head branch when its pull request merges, so a merge is
# what marks those images stale and the next screenshot is what collects them.
# To drop one now:
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

# The nearest bookmark at or behind the working copy, which is the branch this
# change is pushed as. A jj workspace has no `.git`, so git answers only outside
# one.
current_branch() {
	jj log --no-graph --revisions 'heads(::@ & bookmarks())' \
		--template 'bookmarks.map(|bookmark| bookmark.name()).join("\n")' 2>/dev/null |
		grep -m1 . ||
		git branch --show-current
}

REPO="${REPO:-$(origin_url | sed -E 's#.*github\.com[:/]##; s#\.git$##')}"
SLUG="${SLUG:-$(current_branch)}"
[[ -n "$SLUG" ]] || { echo "ERROR: no branch to name the screenshots after; set SLUG" >&2; exit 65; }
BRANCH="screenshots/${SLUG}"

# A screenshot branch is stale once the branch it is named after is gone, which
# for a merged pull request is the moment GitHub deletes its head branch. There
# is nothing to run this on that schedule, so it runs here: posting the next
# screenshot collects the last one's.
prune_stale_screenshots() {
	local branches name
	branches=$(gh api "repos/${REPO}/branches" --paginate --jq '.[].name')
	while read -r name; do
		[[ "$name" == screenshots/* ]] || continue
		grep -qxF "${name#screenshots/}" <<<"$branches" && continue
		gh api --method DELETE "repos/${REPO}/git/refs/heads/${name}" >/dev/null 2>&1 &&
			echo ">>> pruned ${name}" >&2 || true
	done <<<"$branches"
}

prune_stale_screenshots

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

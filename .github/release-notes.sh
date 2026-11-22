#!/bin/sh
# Prints the notes of one version from CHANGELOG.md: the lines under `## <version>` up to the next
# `## ` heading, without blank lines at the start or end. Fails when the section is missing or
# empty, so a release never goes out without notes.
#
#   .github/release-notes.sh 1.4.2 > notes.md
#
# hermes-client-core's update check reads these notes back from the GitHub release (headings,
# `-` lists, nested by indent); `update::changelog_section` follows the same rule and is tested.
set -eu
version="$1"
file="${2:-CHANGELOG.md}"
# `$(...)` drops the blank lines at the end; sed drops the ones at the start.
notes=$(awk -v want="## $version" '
  /^## / { sub(/[ \t\r]+$/, ""); inside = ($0 == want); next }
  inside { sub(/\r$/, ""); print }
' "$file" | sed -e '/./,$!d')
if [ -z "$notes" ]; then
  echo "::error::CHANGELOG.md has no notes under \"## $version\"" >&2
  exit 1
fi
printf '%s\n' "$notes"

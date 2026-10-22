#!/bin/sh
# Reports how far the product is from the documentation, the desired state (docs/README.md).
#
#   scripts/docs-status.sh          one line per page: status, number of Planned markers, path
#   scripts/docs-status.sh --check  the same, and exit 1 when a page breaks the authoring rules
#
# A page breaks the rules when its front matter lacks title, description, status or order,
# when status is not implemented|partial|planned, when it has an H1, or when status disagrees
# with the "> **Planned.**" markers (implemented with a marker; partial or planned without one).
set -eu

cd "$(dirname "$0")/.."
check=0
[ "${1:-}" = "--check" ] && check=1

problems=0
total_planned=0
printf '%-12s %7s  %s\n' STATUS PLANNED PAGE

find docs -name '*.md' ! -path docs/README.md | sort | while read -r page; do
	# Front matter is the block between the first two "---" lines, which must start the file.
	fm=$(awk 'NR==1 && $0!="---" {exit} NR>1 && $0=="---" {exit} NR>1 {print}' "$page")
	field() { printf '%s\n' "$fm" | sed -n "s/^$1: *//p" | head -n 1; }
	status=$(field status)
	planned=$(grep -c '^> \*\*Planned\.\*\*' "$page" || true)
	h1=$(awk '/^```/ {code = !code} !code && f==2 && /^# / {n++} /^---$/ && NR<=12 {f++} END {print n+0}' "$page")
	msg=""
	for key in title description status order; do
		[ -n "$(field "$key")" ] || msg="$msg missing:$key"
	done
	case "$status" in
	implemented) [ "$planned" -eq 0 ] || msg="$msg implemented-with-markers" ;;
	partial) [ "$planned" -gt 0 ] || msg="$msg partial-without-markers" ;;
	planned) [ "$planned" -gt 0 ] || msg="$msg planned-without-marker" ;;
	*) msg="$msg bad-status" ;;
	esac
	[ "$h1" -eq 0 ] || msg="$msg h1"
	printf '%-12s %7s  %s%s\n' "${status:-?}" "$planned" "$page" "${msg:+   <-$msg}"
	[ -z "$msg" ] || echo "$page" >>"${TMPDIR:-/tmp}/docs-status.problems.$$"
	echo "$planned" >>"${TMPDIR:-/tmp}/docs-status.count.$$"
done

count_file="${TMPDIR:-/tmp}/docs-status.count.$$"
problem_file="${TMPDIR:-/tmp}/docs-status.problems.$$"
if [ -f "$count_file" ]; then
	total_planned=$(awk '{s+=$1} END {print s+0}' "$count_file")
	pages=$(wc -l <"$count_file" | tr -d ' ')
	printf '\n%s pages, %s Planned markers left\n' "$pages" "$total_planned"
fi
if [ -f "$problem_file" ]; then
	problems=$(wc -l <"$problem_file" | tr -d ' ')
	printf '%s page(s) break the authoring rules\n' "$problems"
fi
rm -f "$count_file" "$problem_file"

if [ "$check" -eq 1 ] && [ "$problems" -gt 0 ]; then
	exit 1
fi

#!/bin/bash
file=$(jq -r '.tool_input.file_path // empty')
[ -n "$file" ] || exit 0
case "$file" in *.rs) ;; *) exit 0 ;; esac
if ! out=$(rustfmt "$file" 2>&1); then
  echo "$out" >&2
  exit 2
fi

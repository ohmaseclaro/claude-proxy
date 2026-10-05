#!/bin/sh
# Hands a hook event to claude-proxy. Skipped when it is not installed or is a
# version without hooks, which would reject the subcommand and block the tool.
command -v claude-proxy >/dev/null 2>&1 || exit 0
claude-proxy __hook supported >/dev/null 2>&1 || exit 0
exec claude-proxy __hook "$1"

#!/bin/sh
# Prints the delegation policy into each session's context. Silent inside a
# managed run (no runs within runs), when claude-proxy is not installed, or
# when the user opted out with CLAUDE_PROXY_POLICY=off.
[ -n "$CLAUDE_PROXY_RUN" ] && exit 0
[ "$CLAUDE_PROXY_POLICY" = "off" ] && exit 0
command -v claude-proxy >/dev/null 2>&1 || exit 0
cat <<'EOF'
claude-proxy is installed. Standing instructions from the user, until they say otherwise for a task:
- Delegate through claude-proxy managed runs (`claude-proxy run`) instead of the Agent/Task tool or a bare `claude -p` for anything you would give a subagent — including subagents a workflow such as GSD tells you to spawn (`--agent <type>`) — and for any long or big piece of work. Load the `claude-proxy` skill for how.
- Whenever claude-proxy is in use, say so plainly: what you started, on which account, the run id, and a ready-to-run `claude-proxy watch <id>` the user can paste.
EOF

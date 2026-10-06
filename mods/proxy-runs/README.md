# proxy-runs

A Claude Code mod (function-hooks plugin, Claude Code 2.1.287+) that makes
`claude-proxy` managed runs feel like local subagents.

- **A tool in place of Agent.** `mcp__proxy-runs__run` takes Agent's inputs
  (`prompt`, `description`, `subagent_type`, `model`, `isolation: "worktree"`,
  `cwd`, `name`), starts `claude-proxy run - --json …` with the task on stdin
  (`--agent` only for non-built-in types, `-- --model` for the model) and
  returns at once with the run id, its account and `claude-proxy watch <id>`.
- **A live row.** The tool's transcript row draws the run: id, name, state,
  account, turn, cost, what it is doing, and its last transcript lines. The
  `more` / `less` button on the row shows the whole transcript (last 200
  entries).
- **A Runs pane.** `/runs` opens it: this session's runs, the selected run's
  transcript, and allow / allow always / deny / answer / send / kill, plus the
  `claude-proxy attach <id>` line to take one over.
- **A status line.** `2 runs · 1 waiting` while any run is active.
- **Notifications without polling.** At session start the mod follows
  `claude-proxy events --mine`. When a run it started finishes, fails, is
  killed or asks something, it appends a notice row and submits a prompt with
  the result or the question, which starts a turn once the session is idle,
  like a background subagent's task notification.
- **Agent calls redirected.** An `agent.spawn` hook refuses Agent calls and
  points to the tool, unless the description starts with `[direct]`,
  `CLAUDE_PROXY_POLICY=off`, or `CLAUDE_PROXY_RUN` is set. Inside a run the mod
  does nothing at all: no tool, no watcher, no redirect.

## Enable

Terminal, one session:

```console
$ claude --plugin-dir /path/to/claude-proxy/mods/proxy-runs
```

Every session, the desktop app's Code tab included: add the folder to
`CLAUDE_CODE_PLUGIN_DIRS` in the `env` block of `~/.claude/settings.json`:

```json
{ "env": { "CLAUDE_CODE_PLUGIN_DIRS": "/path/to/claude-proxy/mods/proxy-runs" } }
```

Runs inherit that setting (every profile shares `settings.json`), which is why
the mod checks `CLAUDE_PROXY_RUN` and stays inert inside them.

Needs `claude-proxy` 0.7.0+ on `PATH`.

## Limits

- **No native tasks dialog.** The mod API cannot add entries to Claude Code's
  background-tasks dialog; the Runs pane stands in for it.
- **No ctrl+o expansion on the row.** `ToolUse` props carry no `isExpanded` on
  2.1.288 (only `UserMessage` and `ToolGroup` do), so the row has its own
  `more` / `less` button. Whether a click reaches a button in a transcript row
  on each surface is not verified yet.
- **Process access.** `$.process` is declared "CLI only". The desktop Code tab
  runs the CLI, so it should work there, but that is not confirmed. If the
  engine refuses it, the tool refuses with a clear reason, the pane and row
  show a yellow line saying so and read `meta.json` / `events.jsonl` from
  `~/.config/claude-proxy/runs` instead (no asks, re-read every 3 s), and the
  pane's buttons tell you the command to type in a terminal.
- **Headless.** Under `claude -p` the session ends with its turn, so the
  wake-up prompt only arrives in an interactive session or a long-lived SDK /
  stream-json one.
- **Two policies.** With the `quota-router` plugin also enabled, its
  PreToolUse hook turns an Agent call back first (it runs before `agent.spawn`)
  and suggests `claude-proxy run` through Bash; the tool's description tells the
  model to prefer the tool. Its prompt hook also reports finished runs at the
  next prompt, so a run may be mentioned twice.
- Notifications cover runs started through the tool. Runs started through Bash
  still show in the pane and status line; wait on those as before.

## Develop

```console
$ claude plugin validate mods/proxy-runs
$ claude plugin test mods/proxy-runs
```

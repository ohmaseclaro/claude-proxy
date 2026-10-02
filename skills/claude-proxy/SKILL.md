---
name: claude-proxy
description: Spread Claude Code work across the user's Claude accounts by remaining quota. Use whenever you would start a `claude` process from a shell — headless `claude -p` jobs, background or parallel workers, autonomous or GSD runs, scripts — and whenever the user asks which Claude account to use, about Claude quota, usage, or rate limits, or a run hits a usage limit. Run `claude-proxy auto …` instead of `claude …`.
---

# claude-proxy

`claude-proxy` gives each of the user's Claude accounts its own isolated profile
and picks the account with the most quota left. Every account shares the same
skills, plugins (GSD included), hooks, settings, `CLAUDE.md`, and transcripts
(`~/.claude/projects`), so switching accounts changes only whose quota pays.

## The rule

Whenever you would run `claude` from a shell, run `claude-proxy auto` with the
same arguments:

```bash
claude-proxy auto -p "summarize the failing tests"     # instead of: claude -p "..."
claude-proxy auto -p --output-format json "..."        # every claude flag passes through
claude-proxy auto                                      # interactive session for the user
```

Everything after `auto` is forwarded to `claude` unchanged. `auto` reports its
choice on stderr (`claude-proxy: auto → claude-gmail (5h 17% · 7d 22%)`), so
stdout stays clean for `-p` output and pipes.

Prefer `auto` over a fixed `claude-<name>` command. Use a specific account only
when the user asks for that account.

If `claude-proxy` is not installed (`command not found`), use `claude` and tell
the user once.

## Seeing the accounts

```bash
claude-proxy list             # every account and its quota; → marks what auto picks
claude-proxy list --json      # {pick, accounts: [{label, email, logged_in, pressure, windows, error}]}
claude-proxy list --refresh   # ignore the cache
```

- Quota is cached for 15 minutes per account; `list` and `auto` use the cache.
  Do not poll. Use `--refresh` only after a usage-limit error or when the user
  asks for current figures.
- `pressure` is the fullest usage window that has not reset (5-hour, 7-day,
  model-scoped weekly), in percent. `auto` picks the logged-in account with the
  lowest pressure; ties go to the emptier 7-day window.
- `claude (primary)` is the normal `~/.claude` login. It competes too when it is
  logged in.

## When something goes wrong

- **A run hits a usage or rate limit:** `claude-proxy list --refresh`, then rerun
  with `claude-proxy auto` — it moves to another account.
- **`not logged in` or `sign-in expired` for an account:** tell the user to run
  that account's command (e.g. `claude-gmail`) and log in. Never log in for them.
- **`no logged-in account`:** ask the user to add one with
  `claude-proxy add <name>`.

## What `auto` cannot do

`auto` chooses the account for a *new* `claude` process. Subagents started with
the Agent/Task tool run inside the current session and stay on its account. For
long multi-agent work (a whole GSD milestone, an autonomous run), the session
itself should be started with `claude-proxy auto`: suggest that to the user, or
split independent pieces into separate `claude-proxy auto -p` jobs.

## Do not

- Read, print, or copy credentials, Keychain items, or the contents of
  `~/.config/claude-proxy/accounts/*/.claude.json`.
- Run `claude-proxy add` or `remove` unless the user asks; both change their
  setup, and `add` needs them to log in.
- Start many `claude-proxy auto` sessions at once; keep to the user's normal
  concurrency limits.

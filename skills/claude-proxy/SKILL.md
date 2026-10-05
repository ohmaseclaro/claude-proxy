---
name: claude-proxy
description: Delegate work to background Claude sessions you can fully manage — start, follow, message, wait for, and kill them by id — spread across the user's Claude accounts by remaining quota. Use whenever you would start a `claude` process from a shell (headless `claude -p` jobs, background or parallel workers, autonomous or GSD runs, scripts), whenever you want to hand a task to another Claude and keep control of it, and whenever the user asks which Claude account to use, about Claude quota, usage, or rate limits, or a run hits a usage limit.
---

# claude-proxy

`claude-proxy` gives each of the user's Claude accounts its own isolated profile
and picks the account with the most quota left. Every account shares the same
skills, plugins (GSD included), hooks, settings, `CLAUDE.md`, and transcripts
(`~/.claude/projects`), so switching accounts changes only whose quota pays.

On top of that, it runs Claude sessions in the background as **managed runs**:
each gets an id you use to read its transcript, follow it live, check its state,
send it follow-up messages, wait for it, or kill it — like a subagent you own.

If `claude-proxy` is not installed (`command not found`), use `claude` and tell
the user once.

## Delegate a task: managed runs

```bash
id=$(claude-proxy run "fix the failing test in src/parser.rs" -- --permission-mode acceptEdits)
claude-proxy wait "$id" --timeout 900     # blocks; prints the final answer
```

`run` starts the session on the best account and prints only the id. Everything
after `--` goes to `claude` (model, permission mode, allowed tools, …). For a
long task, pass it on stdin: `claude-proxy run - <<'EOF' … EOF`. `--cwd <dir>`
runs it elsewhere; `--name <label>` makes it easy to spot; `--wait` blocks and
prints the answer in one step.

| You want to… | Command |
|---|---|
| block until it is done, get the answer | `claude-proxy wait <id> [--timeout S]` — exit 0 done, 1 failed/killed, 124 timeout |
| know what it is doing now | `claude-proxy status <id>` (`--json` for fields) |
| see recent progress | `claude-proxy tail <id> -n 20` |
| the whole transcript | `claude-proxy read <id>` (`--full` whole tool I/O, `--json` raw events) |
| follow it live until it stops | `claude-proxy watch <id>` |
| only the final answer | `claude-proxy result <id>` |
| give it more instructions | `claude-proxy send <id> "…"` |
| stop it | `claude-proxy kill <id>` |
| see every run | `claude-proxy runs` |
| clean up | `claude-proxy rm <id>` |

How runs behave:

- **States:** `queued` → `working` → `idle` (done; it can take more messages),
  or `failed`, or `killed`. A run is a conversation: each message is one turn of
  the same Claude session.
- **Messages are queued.** `send` while it is working is delivered when the
  current turn ends. `send` to an `idle`, `failed`, or `killed` run resumes the
  same conversation.
- **Waiting:** for anything longer than a minute, start `claude-proxy wait <id>`
  as a background command so you are notified when it finishes, and do other
  work meanwhile. Do not poll `status` in a loop.
- **Context budget:** prefer `status`, `tail -n`, and `result` over `read` on a
  long run; use `read --full` only when you need exact tool output.
- **Permissions:** a background run cannot answer permission prompts — a tool it
  is not allowed to use is simply refused. Pass what the task needs after `--`
  (`--permission-mode acceptEdits`, `--allowedTools "Bash(cargo test:*)"`, …),
  and never more than the user has allowed you.
- **Parallel work:** independent tasks can be separate runs; keep to the user's
  normal concurrency limits.
- **Handing over:** `status` prints the command for the user to open the session
  interactively once it is idle.

## One-shots and interactive sessions: `auto`

When you just need a quick answer inline and don't need to manage the session,
run `claude-proxy auto` with the same arguments you would give `claude`:

```bash
claude-proxy auto -p "summarize this diff" < diff.txt
claude-proxy auto                     # an interactive session for the user
```

`auto` reports its choice on stderr, so stdout stays clean for `-p` output.
Prefer `run` for any delegated work you may want to inspect, message, or stop.

Use a specific account (`--account <name>` on `run`/`send`, or the `claude-<name>`
command) only when the user asks for it.

## Accounts and quota

```bash
claude-proxy list             # every account and its quota; → marks what auto picks
claude-proxy list --json      # {pick, accounts: [{label, email, logged_in, pressure, windows, error}]}
claude-proxy list --refresh   # ignore the cache
```

- Quota is cached for 15 minutes per account, and managed runs refresh it
  from Claude's own live usage reports. Do not poll it. Use `--refresh` only after a usage-limit error or when
  the user asks for current figures.
- `pressure` is the fullest usage window that has not reset (5-hour, 7-day,
  model-scoped weekly), in percent; the logged-in account with the lowest one
  wins. `claude (primary)` is the normal `~/.claude` login and competes too.

## When something goes wrong

- **A run hits a usage or rate limit:** `claude-proxy send <id> --account auto
  "continue"` moves the conversation to the account with the most room and
  picks up where it stopped. For `auto` one-shots: `claude-proxy list
  --refresh`, then retry.
- **`not logged in` or `sign-in expired`:** tell the user to run that account's
  command (e.g. `claude-gmail`) and log in. Never log in for them.
- **`no logged-in account`:** ask the user to add one with
  `claude-proxy add <name>`.
- **A run is `failed`:** `claude-proxy status <id>` and `tail <id>` show why.

## What it cannot do

A run is a separate Claude process. Subagents you start with the Agent/Task tool
run inside your own session and stay on its account. For long multi-agent work
(a whole GSD milestone, an autonomous run), either delegate it as a managed run
or suggest the user start the session itself with `claude-proxy auto`.

## Do not

- Read, print, or copy credentials, Keychain items, or the contents of
  `~/.config/claude-proxy/accounts/*/.claude.json`.
- Run `claude-proxy add` or `remove` unless the user asks; both change their
  setup, and `add` needs them to log in.
- Leave runs working that nobody needs: `kill` what you abandon, `rm` what you
  are done with when the user does not need the record.

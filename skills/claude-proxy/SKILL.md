---
name: claude-proxy
description: Delegate work to background Claude sessions you fully manage by id — start, follow, message, answer, wait for, kill — spread across the user's Claude accounts by remaining quota, with automatic failover when an account hits its limit. Use it by default, instead of the Agent/Task tool, for anything you would hand to a subagent (including subagents a GSD or other workflow tells you to spawn), for any long or big task, and for parallel work — unless the user says not to. Also use it whenever you would start a `claude` process from a shell, and whenever the user asks which Claude account to use or about Claude quota, usage, or rate limits.
---

# claude-proxy

`claude-proxy` runs Claude Code on the user's several Claude accounts. Each
account is its own isolated profile, but all of them share the same skills,
agents, plugins (GSD included), hooks, settings, `CLAUDE.md`, MCP servers, and
transcripts (`~/.claude/projects`) — switching accounts changes only whose quota
pays.

Its **managed runs** are background Claude sessions you control by id, like a
subagent you own: read or follow the transcript, send follow-ups, answer its
questions and permission prompts, wait for it, kill it, or hand it to the user.

If `claude-proxy` is not installed (`command not found`), fall back to your
normal tools and tell the user once.

## When to use it — the default

Unless the user says not to for this task, use a managed run instead of:

- the **Agent/Task tool**, for any work you would give a subagent — research,
  implementation, review, a GSD `gsd-executor`/`gsd-planner`/… (pass the agent
  type with `--agent`);
- doing a **long or big piece of work** inline (multi-file changes, long
  investigations, anything that would eat a lot of your context);
- a bare **`claude -p`** or `claude` process from a shell.

Do the work yourself when it is a few quick tool calls. If `$CLAUDE_PROXY_RUN`
is set, you *are* a managed run: do not start runs of your own unless your task
explicitly asks for it.

## Always disclose it

Whenever claude-proxy is in use, tell the user in plain words — they should
never have to guess. Say what you started, on which account, the run id, and
give a ready-to-run watch command, e.g.:

> Delegated the parser fix to claude-proxy run `a1b2c3` on `claude-gmail`.
> Watch it: `claude-proxy watch a1b2c3` · take over: `claude-proxy attach a1b2c3`

Mention it again when a run moves to another account, asks something, or
finishes. Do this for one-shots (`auto`) too.

## Delegate a task

```bash
id=$(claude-proxy run "fix the failing test in src/parser.rs" -- --permission-mode acceptEdits)
claude-proxy wait "$id"        # run this as a background command
```

`run` starts the session on the best account and prints only the id. Pass a long
task on stdin: `claude-proxy run - <<'EOF' … EOF`. Flags:

| Flag | Use |
|---|---|
| `--agent <type>` | run as one of the user's agents (`~/.claude/agents`), e.g. `gsd-executor` |
| `--worktree` | work in a new git worktree on branch `claude-proxy/<id>` — for parallel runs that touch the same files |
| `--fork` | start from a copy of *your own* conversation (`--fork-from <session-id>` for another one). Costly: the whole context is re-read, with no prompt cache on another account. Use only when the task truly needs what you know |
| `--cwd <dir>` | work somewhere else |
| `--name <label>` | easy to spot in `claude-proxy runs` |
| `--account …` | `auto` (default: best quota, fails over), a name to pin, `a,b` in order. Pin only when the user asked |
| `--wait` | block and print the answer in one step |
| `-- <claude flags>` | anything else for `claude`: `--model`, `--permission-mode`, `--allowedTools`, … |

How the Agent tool maps onto it:

| Agent tool | claude-proxy |
|---|---|
| `prompt` | the task (`-` reads it from stdin) |
| `subagent_type` | `--agent <type>` |
| `run_in_background` + notification | a background `claude-proxy wait <id>` |
| `isolation: worktree` | `--worktree` |
| `model` | `-- --model <model>` |
| the final message | what `wait` (or `result`) prints |
| continuing it | `claude-proxy send <id> "…"` |
| stopping it | `claude-proxy kill <id>` |

## Waiting and answering

Start `claude-proxy wait <id>` as a **background command** and keep working; you
are notified when it exits. Never poll `status` in a loop. Its exit code says
what happened:

| Exit | Meaning | Do |
|---|---|---|
| 0 | done; the answer is on stdout | use it |
| 1 | failed or killed | `claude-proxy status <id>` and `tail <id>` show why |
| 2 | **waiting for an answer**; the question or permission request is on stdout | answer it, then start `wait` again |
| 124 | `--timeout` passed | still running; wait again or check `status` |

A run asks when Claude needs a decision: a **permission prompt** (a tool its
permission mode does not allow) or a **question** (Claude's AskUserQuestion).
Answer with:

```bash
claude-proxy allow <id>                       # let it use the tool
claude-proxy deny <id> "use the staging db"   # refuse; it reads the reason and carries on
claude-proxy answer <id> "Blue"               # one answer per question, in order
claude-proxy deny <id> "decide yourself"      # let it choose
```

Answer yourself when the decision is clearly inside what the user asked for and
what they let you do. Ask the user first — and tell them which run is asking —
for anything they would want a say in: destructive or hard-to-undo actions,
anything outward-facing (pushing, publishing, messaging), credentials, spending,
or a real product decision. Never allow more than the user allowed you. An
unanswered ask times out after 24 hours.

**Permissions:** give a run the same latitude you have, so it does not ask
about routine work — if you may edit files freely, pass
`-- --permission-mode acceptEdits`; add `--allowedTools "Bash(cargo test:*)"`
and the like for commands you would run without asking. Never pass
`bypassPermissions` or `--dangerously-skip-permissions` unless the user asked.

## Everything else you can do

| You want to… | Command |
|---|---|
| know what it is doing now | `claude-proxy status <id>` (`--json` for fields) |
| see recent progress | `claude-proxy tail <id> -n 20` |
| the whole transcript | `claude-proxy read <id>` (`--full` whole tool I/O, `--json` raw events) |
| follow it live | `claude-proxy watch <id>` |
| only the final answer | `claude-proxy result <id>` |
| give it more instructions | `claude-proxy send <id> "…"` (`--account` moves it) |
| stop it | `claude-proxy kill <id>` — the conversation is kept; `send` resumes it |
| hand it to the user | `claude-proxy attach <id>` opens it interactively in their terminal |
| see every run | `claude-proxy runs` |
| clean up | `claude-proxy rm <id>` — also removes its worktree and branch if nothing would be lost |

- **States:** `queued` → `working` (`waiting` while it needs an answer) →
  `idle` (done; it takes more messages), or `failed`, or `killed`.
- **Messages queue.** A `send` while it works is delivered when the turn ends;
  a `send` to an `idle`, `failed`, or `killed` run resumes the same conversation.
- **It moves itself off a full account.** On a usage limit or a sign-in failure
  it continues *the same session* on the next account — `status` shows `moved`,
  the transcript shows `⇄`. Tell the user when it happens.
- **Parallel work:** independent tasks are separate runs, each with its own
  background `wait`; use `--worktree` when they may touch the same files, then
  merge the `claude-proxy/<id>` branches. Keep to the user's concurrency limits.
- **Context budget:** prefer `status`, `tail -n`, and `result` over `read` on a
  long run; `read --full` only when you need exact tool output.
- **Cost of a turn:** each message starts a fresh `claude` process (a few
  seconds), so batch instructions into one message rather than many small ones.

## One-shots: `auto`

For a quick answer you do not need to manage, run `claude-proxy auto` with the
same arguments you would give `claude`:

```bash
claude-proxy auto -p "summarize this diff" < diff.txt
```

`auto` reports its choice on stderr, so stdout stays clean. It chooses once and
does not move if that account hits its limit; prefer `run` for anything you may
want to inspect, message, or stop.

## Accounts and quota

```bash
claude-proxy list             # every account and its quota; → marks what auto picks
claude-proxy list --json      # {pick, accounts: [{label, email, logged_in, pressure, windows, error}]}
claude-proxy list --refresh   # ignore the 15-minute cache
```

`pressure` is the fullest usage window that has not reset (5-hour, 7-day,
model-scoped weekly), in percent; the logged-in account with the lowest wins.
`claude (primary)` is the normal `~/.claude` login and competes too. Runs keep
the cache current from Claude's own usage reports, so do not poll; `--refresh`
only after a usage-limit error or when the user asks.

## When something goes wrong

- **Usage limit:** a run moves on its own; it stays `failed` only when no
  account is left or it was pinned — `claude-proxy send <id> --account auto
  "continue"` moves it by hand.
- **`not logged in` / `sign-in expired`:** tell the user to run that account's
  command (e.g. `claude-gmail`) and log in. Never log in for them.
- **`no logged-in account`:** ask the user to add one with
  `claude-proxy add <name>`.

## Do not

- Read, print, or copy credentials, Keychain items, or
  `~/.config/claude-proxy/accounts/*/.claude.json`.
- Run `claude-proxy add` or `remove` unless the user asks.
- Leave runs working that nobody needs: `kill` what you abandon, `rm` what you
  are done with when the user does not need the record.

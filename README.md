# claude-proxy

Run [Claude Code](https://docs.claude.com/en/docs/claude-code) under more than
one account at the same time — each account becomes its own command, backed by
its own Claude profile — and let `claude-proxy auto` send work to whichever
account has the most quota left.

```console
$ claude-proxy add claude-gmail     # creates a profile, logs it in, installs a `claude-gmail` command
$ claude-gmail                       # a full Claude Code session on that account
$ claude-proxy list                  # every account and its remaining quota
$ claude-proxy auto -p "fix the test"  # runs on the account with the most room
$ claude                             # your original login is untouched
```

One binary. The name you invoke it under decides what it does: run it as
`claude-proxy` and it is the manager; run it as any command it installed
(`claude-gmail`, `work`, …) and it becomes `claude` for that account.

## Why

Claude Code keeps a single logged-in account per machine, so working across a
personal, a work, and a client account means logging in and out — and each one
runs out of quota on its own schedule.

`claude-proxy` gives each account its own command and its own Claude **profile**
(a dedicated `CLAUDE_CONFIG_DIR`). Logins are completely independent: logging one
out, or switching your primary login, never touches another. Everything else is
shared — your skills, plugins, hooks, settings, `CLAUDE.md`, and transcripts —
so every account behaves like your normal `claude`, and all sessions land in
`~/.claude/projects`.

## Install

Requires a Rust toolchain (1.89+) and [Claude Code](https://docs.claude.com/en/docs/claude-code) on your `PATH`.

```console
$ git clone https://github.com/ohmaseclaro/claude-proxy
$ cd claude-proxy
$ cargo install --path .
```

That puts `claude-proxy` in `~/.cargo/bin`. The per-account commands it creates
go in `~/.local/bin`, so make sure that is on your `PATH`:

```sh
export PATH="$HOME/.local/bin:$PATH"
```

`claude-proxy` will tell you if it isn't.

## Usage

### Add an account

```console
$ claude-proxy add claude-gmail
```

This creates an isolated profile and opens **Claude's own login** inside it. Log
in with the account you want this command to use, then type `/exit`.
`claude-proxy` confirms the login landed and installs the `claude-gmail`
command. Each `add` is a separate account — log in as a different one each time.

### Use one account

`claude-gmail` is a transparent proxy — everything after the name goes to
`claude` verbatim:

```console
$ claude-gmail                       # interactive session
$ claude-gmail -p "fix the failing test"
$ claude-gmail /status               # shows this account
```

### See every account's quota

```console
$ claude-proxy list
  claude (primary)  you@example.com  5h 40% (2h10m) · 7d 61% (3d4h)
→ claude-gmail      you@gmail.com    5h 17% (3h34m) · 7d 22% (3d13h)
  claude-work       me@work.com      5h 0% · 7d 95% (4d8h)

→ = what `claude-proxy auto` would use · (…) = time to reset · cached 15 min (--refresh to update)
```

`--json` gives a machine-readable version; `--refresh` skips the cache. The
primary `~/.claude` login is listed and used too, whenever it is logged in.

### Let it pick

```console
$ claude-proxy auto -p "summarize the failing tests"
claude-proxy: auto → claude-gmail (5h 17% (3h34m) · 7d 22% (3d13h))
…
```

`auto` runs `claude` with every argument forwarded, on the logged-in account
with the least **pressure** — the fullest usage window (5-hour, 7-day, or a
model-scoped weekly cap) that has not reset yet. Ties go to the emptier 7-day
window. Its choice is printed on stderr, so stdout stays clean for `-p` output.

`auto` chooses once, at launch, and then hands the terminal to `claude`; if that
account hits its limit mid-session, it does not move. For work that should
survive a limit, use a managed run (below), which fails over on its own.

### Delegate work: managed runs

`run` starts Claude in the background on the best account and prints an id.
From then on the session is yours to manage — read it, follow it, message it,
wait for it, kill it — like a subagent:

```console
$ id=$(claude-proxy run "fix the failing test in src/parser.rs" -- --permission-mode acceptEdits)
$ claude-proxy watch $id                 # follow it live until it stops working
▶ you
  fix the failing test in src/parser.rs
● claude-gmail · turn 1
  ⚙ Bash  cargo test parser
    ↳ test result: FAILED. 11 passed; 1 failed  (+38 lines)
  ⚙ Edit  src/parser.rs
  Fixed the off-by-one in `split_header`; the suite passes now.
✓ done · 6 steps · 48s · $0.31
$ claude-proxy send $id "add a regression test for it"   # continues the same conversation
$ claude-proxy wait $id                  # blocks, prints the final answer
```

| | |
|---|---|
| `claude-proxy run "<task>" [-- <claude flags>]` | start; `-` reads the task from stdin. `--account` (`auto`, a name to pin, or `a,b` in order), `--agent <type>` (one of your agents, e.g. a GSD one), `--worktree`, `--fork` / `--fork-from <session>`, `--name`, `--cwd`, `--wait`, `--json` |
| `claude-proxy status <id>` | state, what it is doing right now, what it is asking, Claude's summary of its last turn, cost (`--json`) |
| `claude-proxy read <id>` | the transcript (`-n N`, `-f` to follow, `--full`, `--json` raw events) |
| `claude-proxy tail <id>` / `watch <id>` | the last entries / follow live until it stops |
| `claude-proxy result <id>` | the final answer of the last turn |
| `claude-proxy send <id> "<message>"` | a follow-up; sent while it works, Claude folds it into the turn in progress. `--account` (`auto`, a name, or `a,b`) moves the run to other accounts |
| `claude-proxy wait <id>` | block until it is done (exit 0 done, 1 failed or killed, 2 waiting for an answer, 124 `--timeout`) |
| `claude-proxy allow <id>` / `deny <id> ["why"]` | settle a permission prompt the run is waiting on; `allow --always`, `--rule "<rule>"`, or `--accept-edits` also stop it asking again |
| `claude-proxy answer <id> "<answer>"…` | answer the question it asked, one answer per question |
| `claude-proxy attach <id>` | open the session interactively in your terminal |
| `claude-proxy kill <id>` | stop the current turn (the conversation is kept; `send` resumes it); on an idle run, free its Claude process |
| `claude-proxy runs` / `rm <id>` | this repository's runs that are going or finished in the last 2 hours, as a tree of who started what (`--all` for every run) / delete one (and its worktree, if clean) |
| `claude-proxy events [<id>…] [--mine]` | a line each time a run finishes, fails, asks, or moves account — made for a Monitor tool |

Each run pins one Claude session, so the conversation carries over — across
Claude processes and onto other accounts, since transcripts are shared. A
background process keeps a headless Claude (`--input-format stream-json`) open
for the run: messages go straight in, one sent mid-turn steers the work in
progress, and a follow-up within five minutes of the last turn
(`CLAUDE_PROXY_IDLE_SECS`) starts without Claude's startup or a cold prompt
cache. Then both exit, so an idle run costs nothing.

**Questions and permission prompts.** When a run needs a decision — a tool its
permission mode does not allow, or a question Claude asks with AskUserQuestion —
it waits, `status` shows `waiting`, and `wait` returns with exit code 2 and
prints what it asks:

```console
$ claude-proxy wait $id
? permission to use Bash  cargo publish --dry-run
    claude-proxy allow 4f2a9c   or   claude-proxy deny 4f2a9c "<reason>"
$ claude-proxy allow $id && claude-proxy wait $id
```

`deny` with a reason tells Claude what to do instead, and
`deny <id> "decide yourself"` lets it choose. Like Claude's own prompt, `allow`
can grant more than the one request: `--always` (that exact command, that
domain, or that tool — for edits, accepting edits), `--rule "Bash(cargo
test:*)"`, or `--accept-edits` (approving a plan). Under the hood, Claude runs
with `--permission-prompt-tool` pointing at a small MCP server inside
`claude-proxy` that records the request and blocks until it is answered (for up
to a day).

A run starts with the permission mode of the Claude session that started it,
the way a subagent inherits it (the plugin's hook records it), unless you pass
one after `--`.

**Worktrees and forks.** `--worktree` creates a git worktree of the current
repository on a new branch, `claude-proxy/<id>`, so parallel runs never edit the
same checkout; `rm` removes the worktree if it has no uncommitted changes, and
the branch if it is merged. `--fork` starts the run from a copy of the
conversation of the Claude session it is called from (`--fork-from <id>` for any
other) — handy for handing off work with its context, but it re-reads the whole
conversation, and on another account without the prompt cache.

**Automatic failover.** When the account a run is on hits a usage limit or
cannot sign in, the run resumes the *same session* on the next account and picks
up where it stopped — nothing is copied, because every profile already sees the
session. If the failed attempt never reached the session (Claude could not even
start), the message is simply sent again; otherwise the new account is asked to
carry on rather than given the message twice. `--account auto` (the default)
moves to the next best account by quota, `--account a,b` tries the list in
order, and a single name pins the run. The transcript marks each move with `⇄`
and `status` shows where it moved from. Failures that are not the account's
fault (an overloaded API, a server error) are not retried elsewhere.

**Old runs clean up after themselves.** A finished run unused for 24 hours is
deleted (`CLAUDE_PROXY_KEEP_HOURS` changes that; `0` keeps everything). Runs
that are going, have messages queued, kept a worktree, or started a run that is
still going are never touched. Looking one up afterwards says when and how it
went, and gives the command to resume its Claude session, which stays.

**Waiting costs nothing.** In your own session, a background
`claude-proxy wait` (or a Monitor on `claude-proxy events --mine`) notifies the
agent when something happens. Inside a run, two things wake it instead of
polling: a run it started sends it a message when that run finishes, fails, or
asks something; and a long command it started in the background (a test suite,
a deploy gate) wakes Claude when it ends — claude-proxy keeps the run's Claude
alive for it and treats the run as working until it is done. The plugin's
hooks also tell your session which of its runs finished or are asking, at each
prompt, and which are still going after a resume or compaction.

Live usage that Claude reports during a run updates that account's quota for
`auto`.

### Remove an account

```console
$ claude-proxy remove claude-gmail
```

This removes the command. The account's profile (and its login) is kept;
`claude-proxy` prints where it is so you can delete it yourself.

## For agents: the `quota-router` plugin

This repository is also a Claude Code plugin. Its skill teaches agents to use
managed runs in place of subagents — for anything they would hand to the Agent
tool (GSD's subagents included, via `--agent`), for long or big work, and for
parallel work — to answer the questions runs ask, and to read
`claude-proxy list`. Its hooks make that the default in every session: the
policy is added to each session's context, ahead of any workflow that says to
use the Agent tool; an Agent call that slips through is turned back with the
equivalent `claude-proxy run`; and each session's permission mode is recorded
for its runs to inherit. The agent tells you whenever it uses `claude-proxy`,
with the run id and a `claude-proxy watch` command you can paste. Install it
once; every profile sees it, since plugins are shared:

```console
$ claude plugin marketplace add ohmaseclaro/claude-proxy
$ claude plugin install quota-router@quota-router
```

Set `CLAUDE_PROXY_POLICY=off` to keep the skill but drop the default; tell an
agent not to use `claude-proxy` and it marks that Agent call `[direct]`. Inside
a run the hooks stay silent, so runs do not start runs of their own.

Agents call `claude-proxy` through their Bash tool, so each call asks for your
permission unless you allow it — add `"Bash(claude-proxy:*)"` to
`permissions.allow` in `~/.claude/settings.json`. That also lets an agent answer
its runs' permission prompts on its own, so allow it only if you would let the
agent take those actions itself.

Known limits: each account asks once to trust a folder the first time you open
it interactively there (runs never ask); `claude-<name> mcp list` does not list
the MCP servers `claude-proxy` passes in, though sessions have them; MCP
servers that need a sign-in ask once per account; and `auto` chooses once —
use `run --wait` for a one-shot that should survive a usage limit.

## How it works

- **One binary, two roles (the busybox pattern).** `claude-proxy` inspects
  `argv[0]`. Invoked under its own name it is the manager; invoked under a name
  it installed it is the proxy. Each command is a symlink to the manager, so
  upgrading `claude-proxy` upgrades every command.
- **Each account is its own Claude profile.** `add` creates a dedicated
  `CLAUDE_CONFIG_DIR` under `~/.config/claude-proxy/accounts/<name>` and runs
  Claude's own login into it; Claude stores and refreshes that account's
  credentials itself.
- **Your setup is shared by symlink.** An allowlist of `~/.claude` entries —
  `settings.json`, `CLAUDE.md`, `skills`, `agents`, `commands`, `plugins`,
  `hooks`, `projects`, and similar, plus get-shit-done's install — is linked
  into each profile. Anything not on the list (the login, the account identity
  in `.claude.json`, org policy, MCP sign-ins, caches) stays per profile. A
  copy a profile made before an entry was shared is merged in (transcripts,
  history) or moved aside as `<name>.pre-shared`, never deleted.
- **So are your MCP servers.** User- and local-scope MCP servers live in
  `~/.claude.json` next to the login, so they cannot be linked; each launch on
  a profile passes them to Claude with `--mcp-config` instead. Servers that
  need a sign-in (OAuth) ask for it once per account.
- **The proxy points Claude at the profile.** It sets `CLAUDE_CONFIG_DIR` and
  clears any inherited credential (`CLAUDE_CODE_OAUTH_TOKEN`,
  `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`) and any host app's session
  identity, so nothing in your environment can override the account, then
  `exec`s `claude` with your arguments.
- **Quota comes from the same endpoint Claude uses.** For each account,
  `claude-proxy` reads that profile's Claude credential, refreshes the access
  token if it is about to expire (writing it back to Claude's own store, under a
  lock), and asks the OAuth usage endpoint. Results are cached for 15 minutes per
  account. The approach is ported from
  [ai-usagebar](https://github.com/akitaonrails/ai-usagebar).

## Security

- Logins happen through Claude's own flow, and Claude stores each account's
  credentials exactly as it does for a normal login (the macOS Keychain, or
  `<profile>/.credentials.json` elsewhere).
- To show quota, `claude-proxy` reads each account's credential and sends the
  access token only to Anthropic's usage and token endpoints. When it refreshes
  an expiring token, it writes the result back to the same store Claude reads,
  so Claude keeps working; it never copies a credential anywhere else, puts one
  on a command line, or prints one. On macOS all Keychain access goes through
  `/usr/bin/security`, the same tool Claude Code uses, so no Keychain dialogs
  appear.
- A proxy clears any inherited credential from the environment before running
  Claude, and the primary `~/.claude` login is never modified.
- MCP server entries can hold API keys, so the files that pass them to a
  profile (`~/.config/claude-proxy/mcp/`, and `mcp.json` in each run) are
  written readable by you only, like `~/.claude.json` itself.
- A run's questions and permission prompts are answered only through
  `claude-proxy allow` / `deny` / `answer`, by whoever can write to your
  `~/.config/claude-proxy`.

If you find a security issue, please open an issue describing the impact.

## Building and testing

```console
$ cargo build
$ cargo test                                 # hermetic — no real HOME, keychain, or network
$ cargo clippy --all-targets -- -D warnings
$ cargo fmt --all -- --check
```

Tests never touch a real home directory, credential store, or the network: path
resolvers, the shared-config linker, and the login check take injected
directories, and quota selection is tested through its pure parser and ranking.
Managed runs are tested end to end (`tests/runs.rs`) against a fake `claude` on
`PATH` in a throwaway home.

## Contributing

Contributions are welcome. Keep changes small and focused, run the four checks
above before opening a pull request, and add a test for any non-trivial logic.
The code aims to be readable first: comments explain the non-obvious, not the
obvious.

## License

MIT. See [LICENSE](LICENSE).

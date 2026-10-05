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
$ claude-proxy send $id "add a regression test for it"   # queued, then resumes the same conversation
$ claude-proxy wait $id                  # blocks, prints the final answer
```

| | |
|---|---|
| `claude-proxy run "<task>" [-- <claude flags>]` | start; `--account` (`auto`, a name to pin, or `a,b` in order), `--name`, `--cwd`, `--wait`, `--json`; `-` reads the task from stdin |
| `claude-proxy status <id>` | state, what it is doing right now, Claude's summary of its last turn, cost (`--json`) |
| `claude-proxy read <id>` | the transcript (`-n N`, `-f` to follow, `--full`, `--json` raw events) |
| `claude-proxy tail <id>` / `watch <id>` | the last entries / follow live until it stops |
| `claude-proxy result <id>` | the final answer of the last turn |
| `claude-proxy send <id> "<message>"` | a follow-up, delivered when the current turn ends; `--account auto` moves the run to another account |
| `claude-proxy wait <id>` | block until it is done (exit 0 done, 1 failed or killed, 124 `--timeout`) |
| `claude-proxy kill <id>` | stop the current turn (the conversation is kept; `send` resumes it) |
| `claude-proxy runs` / `rm <id>` | every run / delete one |

Each run pins one Claude session: every message is one headless turn of it
(`--session-id`, then `--resume`), so the conversation carries over — even onto
another account, since transcripts are shared.

**Automatic failover.** When the account a run is on hits a usage limit or
cannot sign in, the run resumes the *same session* on the next account and picks
up where it stopped — nothing is copied, because every profile already sees the
session. If the failed attempt never reached the session (Claude could not even
start), the message is simply sent again; otherwise the new account is asked to
carry on rather than given the message twice. `--account auto` (the default)
moves to the next best account by quota, `--account a,b` tries the list in
order, and a single name pins the run. The transcript marks each move with `⇄`
and `status` shows where it moved from. Failures that are not the account's
fault (an overloaded API, a server error) are not retried elsewhere. A short-lived background process
works through the run's queued messages and exits when there are none, so an
idle run costs nothing. A background run cannot answer permission prompts, so
pass the permission mode or allowed tools the task needs after `--`. Live usage
that Claude reports during a run updates that account's quota for `auto`.

### Remove an account

```console
$ claude-proxy remove claude-gmail
```

This removes the command. The account's profile (and its login) is kept;
`claude-proxy` prints where it is so you can delete it yourself.

## For agents: the `quota-router` plugin

This repository is also a Claude Code plugin whose skill teaches agents to
delegate work as managed runs they keep control of — and to use `claude-proxy`
instead of `claude` whenever they start a Claude process (headless `-p` jobs,
background workers, GSD or autonomous runs) — plus how to read
`claude-proxy list` and move a run that hits a usage limit. Install it once;
every profile sees it, since plugins are shared:

```console
$ claude plugin marketplace add ohmaseclaro/claude-proxy
$ claude plugin install quota-router@quota-router
```

A run or an `auto` call is a *new* Claude process. Subagents an agent starts inside
its own session stay on that session's account, so for long multi-agent work,
start the session itself with `claude-proxy auto`.

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
  in `.claude.json`, org policy, MCP auth state, caches) stays per profile.
  User-scoped MCP servers live in `~/.claude.json` and are not shared.
- **The proxy points Claude at the profile.** It sets `CLAUDE_CONFIG_DIR` and
  clears any inherited `CLAUDE_CODE_OAUTH_TOKEN` / `ANTHROPIC_API_KEY` /
  `ANTHROPIC_AUTH_TOKEN`, so a credential exported in your shell cannot override
  the account, then `exec`s `claude` with your arguments.
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

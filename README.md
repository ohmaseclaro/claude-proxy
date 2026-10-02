# claude-proxy

Run [Claude Code](https://docs.claude.com/en/docs/claude-code) under more than
one account at the same time — each account becomes its own command, backed by
its own fully independent Claude profile, so they never conflict.

```console
$ claude-proxy add claude-gmail     # creates a profile, logs it in, installs a `claude-gmail` command
$ claude-gmail                       # a full Claude Code session on that account
$ claude-gmail -p "summarize this"   # every claude flag works — it is a transparent proxy
$ claude                             # your original login is completely untouched
```

One binary. The name you invoke it under decides what it does: run it as
`claude-proxy` and it is the manager; run it as any command it installed
(`claude-gmail`, `work`, …) and it becomes `claude` for that account.

## Why

Claude Code keeps a single logged-in account per machine, so working across a
personal, a work, and a client account means logging in and out all day.

`claude-proxy` gives each account its own command and its own Claude
**profile** — a dedicated `CLAUDE_CONFIG_DIR`. Each profile has its own login,
identity, settings, and transcripts, so the accounts are completely independent:
logging one out, or switching your primary login, never touches another. Your
primary `claude` is left entirely alone.

The trade-off of real isolation is that a proxy's transcripts live under its
profile (`~/.config/claude-proxy/accounts/<name>/projects`), not the primary
`~/.claude/projects`.

## Install

Requires a Rust toolchain (1.82+) and [Claude Code](https://docs.claude.com/en/docs/claude-code) on your `PATH`.

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
command.

> Each `add` signs in a separate account — log in as a different one each time.
> Because each proxy has its own profile, `claude-<name> /status` shows that
> command's real account, so you can always confirm which is which.

### Use it

`claude-gmail` is now a command in every shell, and it is a transparent proxy —
everything after the name is forwarded to `claude` verbatim:

```console
$ claude-gmail                       # interactive session
$ claude-gmail -p "fix the failing test"
$ claude-gmail --model opus mcp list
$ claude-gmail /status               # shows this account, in its own profile
```

### List what you have

```console
$ claude-proxy list
  claude-gmail
  work
```

### Remove an account

```console
$ claude-proxy remove claude-gmail
```

This removes the command. The account's profile (its login and transcripts) is
kept; `claude-proxy` prints where it is so you can delete it yourself if you
want.

## How it works

- **One binary, two roles (the busybox pattern).** `claude-proxy` inspects
  `argv[0]`. Invoked under its own name it is the manager; invoked under a name
  it installed it is the proxy. `add` makes a command by copying the binary into
  your bin directory under the chosen name — a copy, not a symlink, so it keeps
  working if the original is moved or upgraded.
- **Each account is its own Claude profile.** `add` creates a dedicated
  `CLAUDE_CONFIG_DIR` and runs Claude's own login into it, so Claude mints,
  stores, and refreshes that account's credentials itself. `claude-proxy` never
  handles a token.
- **The proxy just points Claude at the profile.** It sets `CLAUDE_CONFIG_DIR`
  to the account's directory and clears any inherited `CLAUDE_CODE_OAUTH_TOKEN`
  / `ANTHROPIC_API_KEY` / `ANTHROPIC_AUTH_TOKEN` (so a global credential can't
  override the account), then `exec`s `claude` with your arguments. The config
  dir is the whole mechanism: Claude keys its login, identity, settings, and
  transcripts off it, so the account resolves correctly for both interactive and
  `-p` runs and is unaffected by the primary login. The primary `~/.claude` is
  never modified.

## Security

- `claude-proxy` never handles your credentials. Each account logs in through
  Claude's own flow, and Claude stores the credentials in that profile exactly
  as it does for a normal login. There is no token in `argv`, in the repo, in
  program output, or anywhere `claude-proxy` writes.
- A proxy clears any inherited credential from the environment before running
  Claude, so a `CLAUDE_CODE_OAUTH_TOKEN` exported in your shell cannot silently
  override the account's own login.
- The primary `~/.claude` is never touched.

If you find a security issue, please open an issue describing the impact.

## Building and testing

```console
$ cargo build
$ cargo test                                 # hermetic — no real HOME, keychain, or network
$ cargo clippy --all-targets -- -D warnings
$ cargo fmt --all -- --check
```

Tests never touch a real home directory, credential store, or the network: the
path resolvers and the login check take an injected directory.

## Contributing

Contributions are welcome. Keep changes small and focused, run the four checks
above before opening a pull request, and add a test for any non-trivial logic.
The code aims to be readable first: comments explain the non-obvious, not the
obvious.

## License

MIT. See [LICENSE](LICENSE).

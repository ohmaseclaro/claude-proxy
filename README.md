# claude-proxy

Run [Claude Code](https://docs.claude.com/en/docs/claude-code) under more than
one account at the same time — each account becomes its own command, with its
own credential, and they never conflict.

```console
$ claude-proxy add claude-gmail     # sign in once; installs a `claude-gmail` command
$ claude-gmail                       # a full Claude Code session on that account
$ claude-gmail -p "summarize this"   # every claude flag works — it is a transparent proxy
$ claude                             # your original login is completely untouched
```

One binary. The name you invoke it under decides what it does: run it as
`claude-proxy` and it is the manager; run it as any command it installed
(`claude-gmail`, `work`, …) and it becomes `claude` for that account.

## Why

Claude Code keeps a single logged-in account per machine. If you work across a
personal account, a work account, and a client account, you are stuck logging
in and out. `claude-proxy` gives each account a dedicated command that injects
that account's token for just that run. Nothing is shared, nothing is
clobbered, and your transcripts still land in the usual
`~/.claude/projects` directory because the proxy changes **only** the
credential — not where Claude Code reads and writes.

## Install

### From source

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

This opens your browser to authorize an account, then asks you to paste back
the short code the page shows. `claude-proxy` exchanges that code for a
long-lived token, stores it securely, and copies itself into `~/.local/bin` as
`claude-gmail`. The token is never printed, written to a file, or logged — it
goes straight into the keychain.

> **The account is the one your browser is signed in to at claude.com, not the
> name you chose.** `claude-gmail` is just the command name. If you want this
> command to use a specific account, sign in to that account at claude.com (or
> switch to it) *before* you authorize. To point it at a different account
> later, `claude-proxy remove <name>` and add it again.

> A long-lived token requires a Claude subscription — it is what lets the proxy
> inject it on every run without a refresh step.

### Use it

`claude-gmail` is now a command in every shell, and it is a transparent proxy —
everything after the name is forwarded to `claude` verbatim:

```console
$ claude-gmail                       # interactive session
$ claude-gmail -p "fix the failing test"
$ claude-gmail --model opus mcp list
```

### List what you have

```console
$ claude-proxy list
  claude-gmail
  work

$ claude-proxy list --quota          # also fetch each account's remaining usage
  claude-gmail  [session 13%, weekly 4%]
  work          [token lacks usage scope]
```

### Remove an account

```console
$ claude-proxy remove claude-gmail   # deletes the stored token and the installed command
```

## How it works

- **One binary, two roles (the busybox pattern).** `claude-proxy` inspects
  `argv[0]`. Invoked under its own name it is the manager; invoked under a name
  it installed it is the proxy. `add` makes a command by copying the binary into
  your bin directory under the chosen name — a copy, not a symlink, so it keeps
  working if the original is moved or upgraded.
- **The login runs Claude Code's own OAuth flow.** `add` builds the same
  authorization request the `claude` CLI uses (public client id, `user:inference`
  scope, PKCE S256), opens your browser, and reads back the short code the
  callback page shows. It exchanges that code for the long-lived token over
  HTTPS itself — so, unlike `claude setup-token`, the token is never shown on
  screen.
- **Token injection, nothing else.** The proxy sets
  `CLAUDE_CODE_OAUTH_TOKEN` to that account's token and unsets
  `ANTHROPIC_API_KEY` / `ANTHROPIC_AUTH_TOKEN` (either would override the OAuth
  token and silently bill the wrong account), then `exec`s `claude` with your
  arguments. The default config directory is left alone, so transcripts,
  settings, and MCP config are exactly where `claude` always puts them.
- **Credentials stay out of reach.** On macOS the token lives in the login
  Keychain (via `security`, the same place Claude Code keeps its own). Elsewhere
  it is a mode-`0600` file under `~/.config/claude-proxy`. The token is never
  passed on a command line, never written into the repo, and never printed.

## Security

- Tokens are read from and written to the OS keychain (macOS) or an owner-only
  file (everywhere else). They never appear in `argv`, in environment dumps, or
  in program output. On macOS the keychain write goes through `security -i`
  (the secret on stdin), so it is never an argument and never a terminal prompt.
- The token is obtained by exchanging the OAuth code ourselves, so it is never
  displayed — not even once.
- The proxy injects the token only into the child process's environment and
  leaves `~/.claude` untouched, so it never disturbs your primary login.
- `claude-proxy` never commits secrets. See `.gitignore`.

If you find a security issue, please open an issue describing the impact
(without including any real tokens).

## Building and testing

```console
$ cargo build
$ cargo test                                 # hermetic — no real keychain, HOME, or network
$ cargo clippy --all-targets -- -D warnings
$ cargo fmt --all -- --check
```

Tests never touch a real credential store, home directory, or the network: the
file-backed store and all path resolvers take an injected directory, and the
usage lookup is exercised through its pure response parser.

## Contributing

Contributions are welcome. Keep changes small and focused, run the four checks
above before opening a pull request, and add a test for any non-trivial logic.
The code aims to be readable first: comments explain the non-obvious, not the
obvious.

## License

MIT. See [LICENSE](LICENSE).

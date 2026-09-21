# claude-proxy

Run Claude Code under one of several accounts' OAuth tokens, chosen per
invocation. A generalisation of the single-account `gatik-claude` wrapper.

```
claude-proxy --account=work -p "hello"
```

The token lives in the macOS Keychain and, for the lifetime of one `claude`
process, in that process's environment. It is never written to a file, a shell
profile, a repo, or `~/.claude/settings.json`, and it is never printed back —
not by `config list`, not partially, not anywhere.

## Install

```sh
git clone git@github.com:ohmaseclaro/claude-proxy.git
cd claude-proxy
./install.sh            # symlinks ./claude-proxy into ~/.local/bin
```

No runtime dependencies beyond `security` (macOS) and `claude`.

## Add an account

Mint a token while logged into the account you want to bill:

```sh
claude setup-token      # in a shell logged into that account
claude-proxy config add work
```

`config add` prompts with echo off. **Paste the token as one unbroken line** —
a wrapped paste stores truncated and only fails later, as
`401 OAuth access token is invalid`.

## Use

```sh
claude-proxy --account=work -p "hello"        # --account=x
claude-proxy --account work --model opus      # or --account x
claude-proxy -p "hi"                          # the default account
CLAUDE_PROXY_ACCOUNT=work claude-proxy -p "hi"
```

`--account` may appear anywhere; it and its value are consumed and everything
else is forwarded to `claude` in its original order, so exit codes, signals and
stdio behave exactly as an ordinary `claude` invocation. Arguments after a
literal `--` are passed through untouched, including a later `--account`.

Account resolution order: `--account`, then `$CLAUDE_PROXY_ACCOUNT`, then the
default. No match and no default is an error that lists the accounts that exist.

## Manage

```sh
claude-proxy config list                 # labels, services, whether the Keychain item is there, default
claude-proxy config add <label>          # prompts silently, stores, registers
claude-proxy config rm <label> [-y]      # removes the registry entry AND the Keychain item
claude-proxy config default <label>
```

There is no edit command by design. **To rotate a token**: `config rm <label>`
then `config add <label>` with the fresh one.

Labels must match `[a-z0-9][a-z0-9._-]*`, so they are always safe inside a
Keychain service name and a filename.

## Where things live

| What | Where |
|---|---|
| Registry (labels, service names, default) | `~/.config/claude-proxy/accounts.json`, chmod 600 — never a token |
| Tokens | macOS Keychain, service `claude-proxy-<label>`, account `$USER` |

Read with `security find-generic-password -w`; written with
`security add-generic-password -U -w` taking the value on stdin, so the token
never appears in `ps` or in shell history. `config list` deliberately queries
the Keychain *without* `-w`, so it can say "present" without fetching a secret.

An ordinary `claude` session is unaffected — this is opt-in per invocation.
`ANTHROPIC_API_KEY` and `ANTHROPIC_AUTH_TOKEN` are unset before exec, because
either would take precedence over the OAuth token and silently bill the wrong
account.

## Tests

```sh
test/run-tests.sh
```

54 assertions. They never touch the real Keychain, the real config directory or
the real `claude`: `CLAUDE_PROXY_SECURITY_BIN` points at `test/fake-security`,
`CLAUDE_PROXY_CONFIG_DIR` at a temp dir, and `PATH` is narrowed to a stub
`claude` plus `/usr/bin:/bin`.

## Out of scope

Transcripts, session management, model defaults, per-account settings
directories, anything that edits `~/.claude`. This only decides which
credential `claude` runs under.

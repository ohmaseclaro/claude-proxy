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

No runtime dependencies beyond `security` (macOS) and `claude`. `claude-proxy
list` additionally uses `curl` and `python3`, both of which ship with macOS;
`list --no-quota` works without them.

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

## See what is left

```
$ claude-proxy list
LABEL          DEF  KEYCHAIN  5H     5H RESETS                7D     7D RESETS                OTHER
work           *    present   4%     in 4h 57m (14:23)        10%    in 6d 22h (Sep 28 08:25) Sonnet 12% in 5d 0h (Sep 26 10:25) · Fable 5 31% in 6d 22h (Sep 28 08:25) · extra USD 1.42 of 20.00
research       -    present   77%    in 34m (10:00)           ?      ?                        -
expired        -    present   401 — token rejected: expired, or a setup-token without the usage scope
busy           -    present   429 — rate limited by the usage endpoint
```

One `GET https://api.anthropic.com/api/oauth/usage` per account — no model
request, so it spends nothing. Every field of that response is optional and its
shape varies by plan and over time, so the row shows what came back and nothing
more: the five-hour and seven-day windows, any model-scoped weekly window, and
the pay-as-you-go block when it is enabled. Resets are shown both relatively and
absolutely.

An account whose request fails gets its reason in its own row; the rest of the
table still prints.

```
claude-proxy list --json              # the raw collected payloads, never a token
claude-proxy list --account work
claude-proxy list --timeout 30        # per-account, default 10s
claude-proxy list --no-quota          # labels and Keychain state only, no network
```

**Unverified:** these accounts hold long-lived `claude setup-token` tokens,
while the endpoint is normally called with the rotating credential the Claude
Code client stores. Whether a setup-token carries the usage scope has not been
confirmed against the live endpoint — if it does not, the row says so with the
401.

## Manage

```sh
claude-proxy config list                 # the plain table: labels, services, Keychain state, default
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

The token reaches `curl` through a stdin config file (`-K -`), so it is neither
in `argv` where `ps` would show it, nor ever on disk.

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

95 assertions. They never touch the real Keychain, the real config directory,
the real `claude` or the real API: `CLAUDE_PROXY_SECURITY_BIN` points at
`test/fake-security`, `CLAUDE_PROXY_CONFIG_DIR` at a temp dir,
`CLAUDE_PROXY_USAGE_URL` at `test/fake-usage-server.py` (which picks its
scenario — full payload, sparse payload, malformed JSON, 401, 429, hang — from
the bearer token, so the tests also prove each account is probed with its own
credential), and `PATH` is narrowed to a stub `claude` plus `/usr/bin:/bin`.

## Out of scope

Transcripts, session management, model defaults, per-account settings
directories, anything that edits `~/.claude`. This only decides which
credential `claude` runs under.

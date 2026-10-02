# claude-proxy

Run Claude Code under one of several accounts' OAuth tokens, chosen per
invocation, and see how much quota each account has left.

```console
$ claude-proxy --account=work -p "hello"
$ claude-proxy list
LABEL          DEF  KEYCHAIN  SOURCE    5H     5H RESETS                7D     7D RESETS                OTHER
work           *    present   endpoint  4%     in 4h 57m (14:51)        10%    in 6d 22h (Sep 28 08:53) Sonnet 12% in 5d 0h (Sep 26 10:53) · extra USD 1.42 of 20.00
personal       -    present   -         needs --probe (token lacks the user:profile scope)
old            -    present   -         401 — token rejected: expired or revoked
```

It generalises the common single-account shell wrapper: read a token from the
macOS Keychain, put it in the environment, `exec claude`.

## What it is not

It manages **credentials only**. It does not touch `~/.claude`, and it has no
opinion about transcripts, sessions, models or settings. It decides which
credential `claude` runs under, and nothing else.

## Security model

- **Tokens live in the macOS Keychain and nowhere else.** One generic-password
  item per account. They are never written to the registry file, a shell
  profile, a repo, or `~/.claude/settings.json`.
- **A token never appears in `argv`**, where `ps` would show it to every user on
  the machine. `config add` feeds it to `security` on stdin; the quota request
  feeds the `Authorization` header to `curl` through a stdin config (`curl -K -`).
- **A token is never printed**, not even partially, by any command — including
  `list`, `list --json` and `config list`. Output captured from `claude` is
  redacted against the token before it can reach a row.
- **`ANTHROPIC_API_KEY` and `ANTHROPIC_AUTH_TOKEN` are unset before exec**,
  because either outranks `CLAUDE_CODE_OAUTH_TOKEN` and would silently bill the
  wrong account.
- **Opt-in per invocation.** An ordinary `claude` session is unaffected.
- The registry (`accounts.json`) is chmod 600 and holds labels, Keychain service
  names and which label is the default.

## Requirements

macOS, for `security`. `claude` on `PATH`. `curl` and `python3` — both ship with
macOS — are needed only to read quota in `list`; everything else, including
`list --no-quota`, runs without them.

## Install

```sh
git clone https://github.com/ohmaseclaro/claude-proxy.git
cd claude-proxy
./install.sh                # symlinks ./claude-proxy into ~/.local/bin
BINDIR=/usr/local/bin ./install.sh
```

Or copy the single `claude-proxy` file anywhere on your `PATH`; it has no
repo-relative dependencies.

## Add an account

Mint a token in a shell that is logged into the account you want to bill:

```console
$ claude setup-token
$ claude-proxy config add work
Token for work (input hidden, paste as ONE line): 
Stored 'work' (keychain service claude-proxy-work). It is now the default.
```

The prompt turns echo off. **Paste the token as one unbroken line.** A wrapped
paste stores only the first line, which looks fine here and fails much later as
`401 OAuth access token is invalid`.

Labels must match `[a-z0-9][a-z0-9._-]*`, so a label is always safe inside a
Keychain service name and a filename.

## Commands

### Running claude

```sh
claude-proxy --account=work -p "hello"         # --account=x
claude-proxy --account work --model opus       # or --account x
claude-proxy -p "hi"                           # the default account
CLAUDE_PROXY_ACCOUNT=work claude-proxy -p "hi" # or via the environment
```

`--account` may appear anywhere in the arguments. It and its value are consumed;
everything else is forwarded to `claude` in its original order, with argument
boundaries intact. `claude-proxy` execs `claude`, so exit codes, signals and
stdio behave exactly as a direct invocation.

Everything after a literal `--` is forwarded untouched, including a later
`--account`:

```console
$ claude-proxy --account work -p hi -- --account not-a-flag
# claude receives: -p hi -- --account not-a-flag
```

Account resolution: `--account`, then `$CLAUDE_PROXY_ACCOUNT`, then the default.
With none of those:

```console
$ claude-proxy -p hi
claude-proxy: no account given and no default set. known accounts: personal work
```

### Quota

```console
$ claude-proxy list
LABEL          DEF  KEYCHAIN  SOURCE    5H     5H RESETS                7D     7D RESETS                OTHER
work           *    present   endpoint  4%     in 4h 57m (14:51)        10%    in 6d 22h (Sep 28 08:53) Sonnet 12% in 5d 0h (Sep 26 10:53) · Fable 5 31% in 6d 22h (Sep 28 08:53) · extra USD 1.42 of 20.00
research       -    present   endpoint  77%    in 34m (10:00)           ?      ?                        -
personal       -    present   -         needs --probe (token lacks the user:profile scope)
old            -    present   -         401 — token rejected: expired or revoked
busy           -    present   -         429 — rate limited by the usage endpoint
gone           -    MISSING   -         no Keychain item
```

Default `list` is free and makes no model request. A row that fails prints its
reason and the rest of the table still prints.

```sh
claude-proxy list --probe             # fill in the rows the endpoint could not answer
claude-proxy list --json              # machine-readable, never a token
claude-proxy list --account work      # one account
claude-proxy list --timeout 30        # per-account, default 10s
claude-proxy list --no-quota          # labels and Keychain state only, no network
```

`--no-quota` and `--probe` contradict each other and are refused together.

### Where the numbers come from

There are two sources, and the row says which one it used.

**`endpoint`** — `GET https://api.anthropic.com/api/oauth/usage`, one per
account, no model request, nothing spent. It needs a token scoped
`user:profile`.

**`probe`** — for accounts the endpoint cannot answer. `claude-proxy` runs
`claude -p "." --output-format stream-json --verbose --max-turns 1` under that
account's token, reads stdout until the first `rate_limit_event`, and kills the
child. **This costs one small request against that account**, which is why it is
off by default and announces itself on stderr:

```console
$ claude-proxy list --probe
claude-proxy: --probe: asking claude for personal (one small request).
LABEL          DEF  KEYCHAIN  SOURCE    5H     5H RESETS                7D     7D RESETS                OTHER
personal       -    present   probe     4%     in 4h 57m (14:51)        10%    in 6d 22h (Sep 28 08:53) status allowed · overage rejected (out_of_credits)
```

A `claude setup-token` token — the kind this tool is built around — is **not**
accepted by the usage endpoint. Verified against the live endpoint:

```json
HTTP 403
{"type":"error","error":{"type":"permission_error",
 "message":"OAuth token does not meet scope requirement user:profile",
 "details":{"required_scopes":["user:profile"],"match":"any",
            "error_code":"oauth_scope_insufficient","error_visibility":"user_facing"}}}
```

So for most accounts here, quota means `--probe`. The endpoint path remains for
tokens that do carry `user:profile`.

The two sources report different units — percent and ISO timestamps from the
endpoint, a 0..1 fraction and epoch seconds from the probe. The table
normalises them. `--json` does not: see below.

### Managing accounts

```console
$ claude-proxy config list
LABEL            SERVICE                      KEYCHAIN  DEFAULT
work             claude-proxy-work            present   *
personal         claude-proxy-personal        present   

$ claude-proxy config add <label>        # prompts silently, stores, registers
$ claude-proxy config default <label>    # change the default
$ claude-proxy config rm <label>         # confirms, then removes BOTH the registry entry
Remove account 'old' and its Keychain item 'claude-proxy-old'? [y/N] y
Removed 'old'.
$ claude-proxy config rm <label> -y      # no prompt, for scripts
$ claude-proxy help
```

`config list` is the plain table and never touches the network. `list` is the
richer one.

**There is no edit command, by design.** To rotate a token: `config rm <label>`,
then `config add <label>` with the fresh one.

## For agents and scripting

### `list --json`

Stdout is a JSON array and nothing else; notices and errors go to stderr. One
object per account, in registry order.

| Field | Type | Always | Meaning |
|---|---|---|---|
| `label` | string | yes | the account label |
| `default` | bool | yes | whether it is the default account |
| `keychain` | `"present"` \| `"MISSING"` | yes | whether its Keychain item exists |
| `source` | `"endpoint"` \| `"probe"` | only on success | where the quota came from |
| `usage` | object | only on success | the **raw** payload from that source |
| `error` | string | only on failure | why the quota could not be read |

`usage` is passed through verbatim, so its shape depends on `source`:

- `source: "endpoint"` → the usage endpoint's own object: `five_hour`,
  `seven_day`, `seven_day_sonnet` (each `{utilization, resets_at}` where
  **utilization is 0..100** and `resets_at` is **ISO-8601**), `extra_usage`
  (money in minor units), and a `limits[]` array whose `weekly_scoped` entries
  carry per-model caps.
- `source: "probe"` → the whole `rate_limit_event` object, whose
  `rate_limit_info.unifiedWindows.<window>` carries **utilization as a 0..1
  fraction** and **`resetsAt` as epoch seconds**.

Every field of both payloads is optional and the shapes drift; do not assume a
key exists.

```console
$ claude-proxy list --json --account=work
[
  {"label": "work", "default": true, "keychain": "present", "source": "endpoint", "usage": {"five_hour": {"utilization": 4, "resets_at": "2026-09-21T14:51:00Z"}, ...}}
]
$ claude-proxy list --json --account=personal
[
  {"label": "personal", "default": false, "keychain": "present", "error": "needs --probe (token lacks the user:profile scope)"}
]
```

### Exit codes

| Code | Meaning |
|---|---|
| 0 | success — **including a `list` where some or all rows failed** |
| 1 | claude-proxy's own error: unknown account, no default, bad label, bad flag, missing token, `claude` not on `PATH`, or a declined `config rm` |
| anything else | came from `claude` itself, passed through unchanged by `exec` |

A run that reaches `claude` returns `claude`'s exit status, so a non-zero code
is not necessarily this tool's. To decide anything about quota, read
`list --json` and check each object's `error`, not the exit code.

### Environment

| Variable | Effect |
|---|---|
| `CLAUDE_PROXY_ACCOUNT` | account to use when `--account` is absent |
| `CLAUDE_PROXY_CONFIG_DIR` | registry directory (default `~/.config/claude-proxy`) |
| `CLAUDE_PROXY_SECURITY_BIN` | the `security` binary to call (tests use a fake) |
| `CLAUDE_PROXY_USAGE_URL` | the usage endpoint (tests point it at a local server) |
| `CLAUDE_PROXY_PYTHON` | the `python3` to parse responses with |

`CLAUDE_CODE_OAUTH_TOKEN` is set by this tool for the process it runs; setting
it yourself has no effect here. `ANTHROPIC_API_KEY` and `ANTHROPIC_AUTH_TOKEN`
are dropped. The probe uses whatever `claude` is on `PATH`.

### Non-interactive account management

`config add` reads the token from stdin when stdin is not a terminal, so nothing
has to be typed and no token reaches `argv`:

```sh
printf '%s\n' "$TOKEN" | claude-proxy config add work   # from a variable
pbpaste | claude-proxy config add work                  # from the clipboard
claude-proxy config rm work -y                          # no prompt
```

Keep the token out of your shell history: read it from a pipe or a process
substitution, never as a literal argument.

### Worked example: run under whichever account has the most headroom

```sh
#!/bin/sh
# Pick the account with the lowest five-hour utilization, then run claude there.
best=$(claude-proxy list --probe --json 2>/dev/null | python3 -c '
import json, sys
best, best_used = None, None
for row in json.load(sys.stdin):
    if row.get("error"):
        continue
    u = row.get("usage") or {}
    if row.get("source") == "probe":
        w = ((u.get("rate_limit_info") or {}).get("unifiedWindows") or {}).get("five_hour") or {}
        used = w.get("utilization")
        used = None if used is None else used * 100      # 0..1 fraction
    else:
        used = ((u.get("five_hour") or {}).get("utilization"))   # already 0..100
    if used is None:
        continue
    if best_used is None or used < best_used:
        best, best_used = row["label"], used
print(best or "")')

[ -n "$best" ] || { echo "no account has readable quota" >&2; exit 1; }
exec claude-proxy --account "$best" "$@"
```

## Where things live

| What | Where |
|---|---|
| Registry: labels, service names, default | `~/.config/claude-proxy/accounts.json`, chmod 600 — never a token |
| Tokens | macOS Keychain, service `claude-proxy-<label>`, account `$USER` |

```json
{
  "default": "work",
  "accounts": [
    { "label": "work", "service": "claude-proxy-work" },
    { "label": "personal", "service": "claude-proxy-personal" }
  ]
}
```

Inspect an item yourself with
`security find-generic-password -a "$USER" -s claude-proxy-work` (add `-w` to
print the secret, which this tool never does).

## Known limits

- **macOS only.** It depends on `security`; there is no Linux secret-store path.
- **One Keychain item per account**, named from the label. Rename means remove
  and add.
- **`list`, `config` and `help` as the first argument are intercepted**, so
  `claude`'s own subcommands of those names are shadowed. Everything else, and
  anything after `--`, passes through.
- **No edit command** by design — rotation is remove-then-add.
- **`list` exits 0 even when every row failed.** Check `error` in `--json`.
- The probe reads the first `rate_limit_event` and kills `claude`; it does not
  wait for the turn to finish.

## Tests

```sh
test/run-tests.sh
```

129 assertions, no framework. They never touch the real Keychain, the real
config directory, the real `claude` or the real API:

- `CLAUDE_PROXY_SECURITY_BIN` → `test/fake-security`, a file-backed stand-in.
- `CLAUDE_PROXY_CONFIG_DIR` → a temp directory.
- `CLAUDE_PROXY_USAGE_URL` → `test/fake-usage-server.py`, which picks its
  scenario (full payload, sparse payload, malformed JSON, 403-scope, 401, 429,
  hang) from the bearer token, so the tests also prove each account is probed
  with its own credential. It answers 429 unless the request carries a
  `claude-code/` User-Agent, which the live endpoint also requires.
- `PATH` → a stub `claude` plus `/usr/bin:/bin`, so the real binary is
  unreachable. The stub plays scripted `stream-json` scenarios for the probe
  path and otherwise echoes its arguments.

## Licence

MIT — see [LICENSE](LICENSE).

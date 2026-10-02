#!/bin/sh
# claude-proxy — run Claude Code under one of several accounts' OAuth tokens.
#
# The token lives only in the macOS Keychain and in the environment of the
# exec'd `claude`. It is never written to a file and never printed back.
#
#   claude-proxy config add work      # paste the token once, silently
#   claude-proxy --account=work -p "hello"
#
# Mint a token with `claude setup-token` while logged into that account, and
# paste it as ONE unbroken line: a wrapped paste stores truncated and fails
# later as `401 OAuth access token is invalid`.
set -eu

PROG=claude-proxy
CONFIG_DIR=${CLAUDE_PROXY_CONFIG_DIR:-$HOME/.config/claude-proxy}
REGISTRY=$CONFIG_DIR/accounts.json
SECURITY=${CLAUDE_PROXY_SECURITY_BIN:-security}
KC_ACCOUNT=${USER:-$(id -un)}
USAGE_URL=${CLAUDE_PROXY_USAGE_URL:-https://api.anthropic.com/api/oauth/usage}
PYTHON=${CLAUDE_PROXY_PYTHON:-python3}

die() { printf '%s: %s\n' "$PROG" "$1" >&2; exit 1; }

usage() {
  cat <<'USAGE'
usage: claude-proxy [--account <label>] [claude args...]
       claude-proxy list [--no-quota] [--json] [--account <label>] [--timeout <s>]
       claude-proxy config list
       claude-proxy config add <label>
       claude-proxy config rm <label> [-y]
       claude-proxy config default <label>

Everything that is not --account is forwarded to `claude` unchanged.
Account resolution: --account, then $CLAUDE_PROXY_ACCOUNT, then the default.
USAGE
}

service_for() { printf 'claude-proxy-%s' "$1"; }

valid_label() {
  printf '%s' "$1" | LC_ALL=C grep -qx '[a-z0-9][a-z0-9._-]*'
}

registry_labels() {
  [ -f "$REGISTRY" ] || return 0
  sed -n 's/^[[:space:]]*{ *"label": *"\([^"]*\)".*/\1/p' "$REGISTRY"
}

registry_default() {
  [ -f "$REGISTRY" ] || return 0
  sed -n 's/^[[:space:]]*"default": *"\([^"]*\)".*/\1/p' "$REGISTRY" | head -n 1
}

registry_has() {
  registry_labels | grep -qxF "$1"
}

registry_service() {
  svc=$(sed -n 's/^.*"label": *"'"$1"'" *, *"service": *"\([^"]*\)".*/\1/p' "$REGISTRY" 2>/dev/null | head -n 1)
  [ -n "$svc" ] || svc=$(service_for "$1")
  printf '%s' "$svc"
}

known_accounts_msg() {
  labels=$(registry_labels)
  if [ -z "$labels" ]; then
    printf 'no accounts configured. Add one with: %s config add <label>' "$PROG"
  else
    printf 'known accounts: %s' "$(printf '%s\n' "$labels" | tr '\n' ' ' | sed 's/ $//')"
  fi
}

write_registry() {
  # $1 = default label (may be empty), $2 = newline-separated labels
  _def=$1
  _entries=$(printf '%s\n' "$2" | grep -v '^[[:space:]]*$' \
    | sed 's/.*/    { "label": "&", "service": "claude-proxy-&" },/' | sed '$ s/,$//')
  mkdir -p "$CONFIG_DIR"
  chmod 700 "$CONFIG_DIR" 2>/dev/null || true
  _tmp=$REGISTRY.tmp.$$
  umask 077
  {
    printf '{\n'
    if [ -n "$_def" ]; then printf '  "default": "%s",\n' "$_def"; else printf '  "default": null,\n'; fi
    printf '  "accounts": [\n'
    [ -n "$_entries" ] && printf '%s\n' "$_entries"
    printf '  ]\n}\n'
  } > "$_tmp"
  chmod 600 "$_tmp"
  mv "$_tmp" "$REGISTRY"
}

cmd_list() {
  labels=$(registry_labels)
  if [ -z "$labels" ]; then
    printf 'No accounts configured. Add one with: %s config add <label>\n' "$PROG"
    return 0
  fi
  def=$(registry_default)
  printf '%-16s %-28s %-9s %s\n' LABEL SERVICE KEYCHAIN DEFAULT
  printf '%s\n' "$labels" | while IFS= read -r l; do
    [ -n "$l" ] || continue
    svc=$(registry_service "$l")
    # Metadata only — never -w, so no token is ever read here, let alone printed.
    if "$SECURITY" find-generic-password -a "$KC_ACCOUNT" -s "$svc" >/dev/null 2>&1; then
      kc=present
    else
      kc=MISSING
    fi
    [ "$l" = "$def" ] && mark='*' || mark=''
    printf '%-16s %-28s %-9s %s\n' "$l" "$svc" "$kc" "$mark"
  done
}

cmd_add() {
  label=${1:-}
  [ -n "$label" ] || die 'config add needs a label'
  valid_label "$label" || die "invalid label '$label': use [a-z0-9][a-z0-9._-]*"
  registry_has "$label" && die "account '$label' already exists; remove it first ($PROG config rm $label)"

  svc=$(service_for "$label")
  printf 'Token for %s (input hidden, paste as ONE line): ' "$label" >&2
  if stty_saved=$(stty -g 2>/dev/null); then
    stty -echo 2>/dev/null || true
    IFS= read -r token || token=''
    stty "$stty_saved" 2>/dev/null || true
    printf '\n' >&2
  else
    IFS= read -r token || token=''
  fi
  [ -n "$token" ] || die 'empty token, nothing stored'

  # Value on stdin, never on the command line, so it stays out of `ps`.
  printf '%s' "$token" | "$SECURITY" add-generic-password -a "$KC_ACCOUNT" -s "$svc" -U -w \
    || die "could not store the token in the Keychain under '$svc'"
  token=''
  unset token

  labels=$(registry_labels; printf '%s\n' "$label")
  def=$(registry_default)
  [ -n "$def" ] || def=$label
  write_registry "$def" "$labels"
  printf "Stored '%s' (keychain service %s).%s\n" "$label" "$svc" \
    "$([ "$def" = "$label" ] && printf ' It is now the default.')"
}

cmd_rm() {
  label=${1:-}
  yes=${2:-}
  [ -n "$label" ] || die 'config rm needs a label'
  registry_has "$label" || die "unknown account '$label': $(known_accounts_msg)"
  svc=$(registry_service "$label")

  if [ "$yes" != "-y" ] && [ "$yes" != "--yes" ]; then
    printf "Remove account '%s' and its Keychain item '%s'? [y/N] " "$label" "$svc" >&2
    IFS= read -r reply || reply=''
    case $reply in y|Y|yes|YES) ;; *) printf 'Aborted.\n' >&2; exit 1 ;; esac
  fi

  "$SECURITY" delete-generic-password -a "$KC_ACCOUNT" -s "$svc" >/dev/null 2>&1 \
    || printf '%s: no Keychain item for %s (already gone)\n' "$PROG" "$svc" >&2

  labels=$(registry_labels | grep -vxF "$label" || true)
  def=$(registry_default)
  [ "$def" = "$label" ] && def=''
  write_registry "$def" "$labels"
  printf "Removed '%s'.\n" "$label"
}

cmd_default() {
  label=${1:-}
  [ -n "$label" ] || die 'config default needs a label'
  registry_has "$label" || die "unknown account '$label': $(known_accounts_msg)"
  write_registry "$label" "$(registry_labels)"
  printf "Default account is now '%s'.\n" "$label"
}

list_usage() {
  cat <<'USAGE'
usage: claude-proxy list [--probe] [--json] [--account <label>] [--timeout <s>]
                         [--no-quota]

Shows every configured account with its quota, read for free from the Anthropic
OAuth usage endpoint: one GET per account, no model request, nothing spent.

That endpoint needs a token scoped `user:profile`. A `claude setup-token`
token is not, and answers 403 — those accounts read as "needs --probe".

  --probe             ask `claude` itself for the quota of the accounts the
                      endpoint could not answer. COSTS ONE SMALL REQUEST per
                      such account. Off by default.
  --json              the collected data as JSON (never a token)
  --account <label>   only this account
  --timeout <s>       give up on one read after s seconds (default 10)
  --no-quota          skip the network entirely: labels and Keychain state only
USAGE
}

jesc() { printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'; }

# Both sources flatten to the same key=value records.
read_flat() {
  while IFS='=' read -r k v; do
    case $k in
      five_hour.percent) P_5=$v ;;
      five_hour.resets_epoch) P_5R=$v ;;
      seven_day.percent) P_7=$v ;;
      seven_day.resets_epoch) P_7R=$v ;;
      scoped)
        n=${v%%|*}; rest=${v#*|}; sp=${rest%%|*}; se=${rest##*|}
        P_TAIL="${P_TAIL:+$P_TAIL · }$n $(fmt_pct "$sp") $(when "$se")" ;;
      tail) P_TAIL="${P_TAIL:+$P_TAIL · }$v" ;;
      error) P_ERR=$v ;;
    esac
  done < "$1"
}

fmt_pct() {
  [ -n "$1" ] || { printf '?'; return; }
  awk -v p="$1" 'BEGIN { if (p > 0 && p < 1) printf "<1%%"; else printf "%.0f%%", p }' 2>/dev/null || printf '?'
}

when() {
  e=$1
  [ -n "$e" ] || { printf '?'; return; }
  case $e in *[!0-9]*) printf '?'; return ;; esac
  now=$(date +%s)
  d=$((e - now))
  if [ "$d" -lt 0 ]; then rel=overdue
  elif [ "$d" -lt 60 ]; then rel='in <1m'
  elif [ "$d" -lt 3600 ]; then rel="in $((d / 60))m"
  elif [ "$d" -lt 86400 ]; then rel="in $((d / 3600))h $((d % 3600 / 60))m"
  elif [ "$d" -lt 8640000 ]; then rel="in $((d / 86400))d $((d % 86400 / 3600))h"
  else rel='in >99d'
  fi
  day=$(date -r "$e" '+%F' 2>/dev/null || date -d "@$e" '+%F' 2>/dev/null || true)
  if [ -n "$day" ] && [ "$day" = "$(date '+%F')" ]; then
    abs=$(date -r "$e" '+%H:%M' 2>/dev/null || date -d "@$e" '+%H:%M' 2>/dev/null || true)
  else
    abs=$(date -r "$e" '+%b %d %H:%M' 2>/dev/null || date -d "@$e" '+%b %d %H:%M' 2>/dev/null || true)
  fi
  if [ -n "$abs" ]; then printf '%s (%s)' "$rel" "$abs"; else printf '%s' "$rel"; fi
}

# Every field of this response is optional and the shape drifts between plans
# and releases, so flatten defensively and render whatever came back.
FLATTEN=$(cat <<'PYEOF'
import json, sys, datetime

def epoch(s):
    try:
        d = datetime.datetime.fromisoformat(str(s).replace("Z", "+00:00"))
        if d.tzinfo is None:
            d = d.replace(tzinfo=datetime.timezone.utc)
        return int(d.timestamp())
    except Exception:
        return ""

def num(v):
    return v if isinstance(v, (int, float)) and not isinstance(v, bool) else None

try:
    doc = json.load(open(sys.argv[1]))
except Exception:
    print("error=unreadable usage response")
    sys.exit(0)
if not isinstance(doc, dict):
    print("error=unexpected usage response")
    sys.exit(0)

def window(obj):
    if not isinstance(obj, dict):
        return None, ""
    return num(obj.get("utilization")), epoch(obj.get("resets_at"))

for key in ("five_hour", "seven_day"):
    pct, at = window(doc.get(key))
    if pct is not None:
        print("%s.percent=%s" % (key, pct))
    if at != "":
        print("%s.resets_epoch=%s" % (key, at))

seen = set()
def scoped(name, pct, at):
    if not name or name.lower() in seen or pct is None:
        return
    seen.add(name.lower())
    print("scoped=%s|%s|%s" % (name.replace("|", "/"), pct, at))

pct, at = window(doc.get("seven_day_sonnet"))
scoped("Sonnet", pct, at)

for entry in doc.get("limits") or []:
    if not isinstance(entry, dict) or entry.get("kind") != "weekly_scoped":
        continue
    model = ((entry.get("scope") or {}).get("model") or {}) if isinstance(entry.get("scope"), dict) else {}
    scoped(model.get("display_name"), num(entry.get("percent")), epoch(entry.get("resets_at")))

extra = doc.get("extra_usage")
if isinstance(extra, dict) and extra.get("is_enabled"):
    dp = extra.get("decimal_places")
    dp = dp if isinstance(dp, int) and 0 <= dp <= 6 else 2
    cur = extra.get("currency") or ""
    def money(v):
        v = num(v)
        return None if v is None else ("%.*f" % (dp, v / (10 ** dp)))
    used, cap = money(extra.get("used_credits")), money(extra.get("monthly_limit"))
    if used is not None and cap is not None:
        print("tail=extra %s %s of %s" % (cur, used, cap))
    elif used is not None:
        print("tail=extra %s %s used" % (cur, used))
    else:
        print("tail=extra usage on")
PYEOF
)

# The stream-json rate_limit_event says the same things in different units:
# utilization is a 0..1 fraction and resetsAt is epoch seconds. Normalise to
# the endpoint's shape so one table can mix both sources.
PROBE_FLATTEN=$(cat <<'PYEOF'
import json, sys

def num(v):
    return v if isinstance(v, (int, float)) and not isinstance(v, bool) else None

try:
    info = json.load(open(sys.argv[1]))["rate_limit_info"]
    if not isinstance(info, dict):
        raise ValueError
except Exception:
    print("error=unreadable rate_limit_event")
    sys.exit(0)

def window(name, obj):
    if not isinstance(obj, dict):
        return None, None
    pct, at = num(obj.get("utilization")), num(obj.get("resetsAt"))
    return (None if pct is None else pct * 100), (None if at is None else int(at))

windows = info.get("unifiedWindows")
windows = windows if isinstance(windows, dict) else {}
for key in ("five_hour", "seven_day"):
    pct, at = window(key, windows.get(key))
    if pct is not None:
        print("%s.percent=%s" % (key, pct))
    if at is not None:
        print("%s.resets_epoch=%s" % (key, at))

for key, obj in windows.items():
    if key in ("five_hour", "seven_day"):
        continue
    pct, at = window(key, obj)
    if pct is not None:
        print("scoped=%s|%s|%s" % (key.replace("|", "/"), pct, "" if at is None else at))

bits = []
if info.get("status"):
    bits.append("status %s" % info["status"])
if info.get("isUsingOverage") is True:
    bits.append("overage in use")
elif info.get("overageStatus"):
    over = "overage %s" % info["overageStatus"]
    if info.get("overageDisabledReason"):
        over += " (%s)" % info["overageDisabledReason"]
    bits.append(over)
if bits:
    print("tail=%s" % " · ".join(bits))
PYEOF
)

# The token goes to curl through a stdin config file: not in argv (where `ps`
# would show it) and not on disk.
usage_fetch() {
  _tok=$1; _timeout=$2; _body=$3
  printf 'header = "Authorization: Bearer %s"\n' "$_tok" \
    | curl -sS -K - \
        -H 'anthropic-beta: oauth-2025-04-20' \
        -H 'User-Agent: claude-code/2.1.183' \
        -H 'Content-Type: application/json' \
        --max-time "$_timeout" \
        -o "$_body" -w '%{http_code}' \
        "$USAGE_URL" 2>"$_body.err"
}

account_token() {
  "$SECURITY" find-generic-password -a "$KC_ACCOUNT" -s "$(registry_service "$1")" -w 2>/dev/null || true
}

read_endpoint() {
  label=$1 timeout=$2
  tok=$(account_token "$label")
  [ -n "$tok" ] || { P_ERR='no Keychain item'; P_NOKEY=1; return 1; }

  body=$WORK/body
  curl_st=0
  code=$(usage_fetch "$tok" "$timeout" "$body") || curl_st=$?
  tok=''; unset tok

  if [ "$curl_st" != 0 ]; then
    case $curl_st in
      28) P_ERR="timed out after ${timeout}s" ;;
      6)  P_ERR='cannot resolve the API host' ;;
      7)  P_ERR='cannot reach the API host' ;;
      *)  P_ERR="network error (curl exit $curl_st): $(head -n 1 "$body.err" 2>/dev/null | cut -c1-90)" ;;
    esac
    return 1
  fi

  case $code in
    2*) ;;
    403)
      # The expected, ordinary answer for a setup-token: not something to fix.
      if grep -q 'oauth_scope_insufficient\|user:profile' "$body" 2>/dev/null; then
        P_ERR='needs --probe (token lacks the user:profile scope)'
      else
        P_ERR="403 — forbidden: $(tr -d '\n' < "$body" 2>/dev/null | cut -c1-80)"
      fi
      return 1 ;;
    401)
      P_ERR='401 — token rejected: expired or revoked'
      return 1 ;;
    429)
      P_ERR='429 — rate limited by the usage endpoint'
      return 1 ;;
    *)
      P_ERR="HTTP ${code:-?}: $(tr -d '\n' < "$body" 2>/dev/null | cut -c1-90)"
      return 1 ;;
  esac

  flat=$WORK/flat
  printf '%s' "$FLATTEN" | "$PYTHON" - "$body" > "$flat" 2>/dev/null || {
    P_ERR='could not parse the usage response'; return 1; }
  P_BODY=$body

  read_flat "$flat"

  [ -z "$P_ERR" ] || return 1
  if [ -z "$P_5" ] && [ -z "$P_7" ] && [ -z "$P_TAIL" ]; then
    P_ERR='usage response carried no windows'
    return 1
  fi
  P_SOURCE=endpoint
  return 0
}

# --- the probe: the only quota source a setup-token has ---------------------

spawn() {
  _o=$1; _e=$2; _d=$3; shift 3
  rm -f "$_d"
  ( claude "$@" >"$_o" 2>"$_e"; printf '%s' "$?" >"$_d" ) 2>/dev/null &
  SPAWN_PID=$!
}

# 0 = pattern seen, 1 = child finished, 2 = timed out.
await() {
  _pid=$1; _done=$2; _out=$3; _pat=$4; _ticks=$(( $5 * 4 ))
  while [ "$_ticks" -gt 0 ]; do
    if [ -n "$_pat" ] && grep -q "$_pat" "$_out" 2>/dev/null; then return 0; fi
    [ -f "$_done" ] && return 1
    sleep 0.25
    _ticks=$((_ticks - 1))
  done
  return 2
}

reap() {
  pkill -P "$1" >/dev/null 2>&1 || true
  kill "$1" >/dev/null 2>&1 || true
  wait "$1" >/dev/null 2>&1 || true
}

# A chatty CLI must not be able to put the token into a row we then print.
redact() {
  awk -v t="$2" '{ if (t != "") { i = index($0, t); while (i > 0) { $0 = substr($0, 1, i - 1) "<redacted>" substr($0, i + length(t)); i = index($0, t) } } print }' <<REDACT
$1
REDACT
}

probe_claude() {
  label=$1 timeout=$2
  P_ERR=''; P_5=''; P_5R=''; P_7=''; P_7R=''; P_TAIL=''; P_BODY=''; P_SOURCE=''

  command -v claude >/dev/null 2>&1 || { P_ERR='probe: claude is not on PATH'; return 1; }
  tok=$(account_token "$label")
  [ -n "$tok" ] || { P_ERR='no Keychain item'; P_NOKEY=1; return 1; }

  printf '%s: --probe: asking claude for %s (one small request).\n' "$PROG" "$label" >&2

  out=$WORK/probe.out; err=$WORK/probe.err; fin=$WORK/probe.done
  CLAUDE_CODE_OAUTH_TOKEN=$tok
  export CLAUDE_CODE_OAUTH_TOKEN
  spawn "$out" "$err" "$fin" -p . --output-format stream-json --verbose --max-turns 1
  await "$SPAWN_PID" "$fin" "$out" '"rate_limit_info"' "$timeout"
  st=$?
  reap "$SPAWN_PID"
  CLAUDE_CODE_OAUTH_TOKEN=''
  unset CLAUDE_CODE_OAUTH_TOKEN

  line=$(grep -m 1 '"rate_limit_info"' "$out" 2>/dev/null || true)
  if [ -z "$line" ]; then
    if [ "$st" = 2 ]; then
      P_ERR="probe: timed out after ${timeout}s"
    else
      code=$(cat "$fin" 2>/dev/null || true)
      if [ "${code:-0}" = 0 ]; then
        P_ERR='probe: no rate_limit_event in the response'
      else
        why=$(head -n 1 "$err" 2>/dev/null || true)
        [ -n "$why" ] || why=$(head -n 1 "$out" 2>/dev/null || true)
        why=$(redact "$why" "$tok" | cut -c1-100)
        P_ERR="probe failed (exit $code)${why:+: $why}"
      fi
    fi
    tok=''; unset tok
    return 1
  fi
  tok=''; unset tok

  event=$WORK/event.json
  printf '%s\n' "$line" > "$event"
  flat=$WORK/probe.flat
  printf '%s' "$PROBE_FLATTEN" | "$PYTHON" - "$event" > "$flat" 2>/dev/null || {
    P_ERR='probe: could not parse the rate_limit_event'; return 1; }
  read_flat "$flat"
  [ -z "$P_ERR" ] || { P_ERR="probe: $P_ERR"; return 1; }
  if [ -z "$P_5" ] && [ -z "$P_7" ] && [ -z "$P_TAIL" ]; then
    P_ERR='probe: rate_limit_event carried no windows'
    return 1
  fi
  P_BODY=$event
  P_SOURCE=probe
  return 0
}

cmd_list_rich() {
  quota=1 as_json=0 only='' timeout=10 do_probe=0
  while [ $# -gt 0 ]; do
    case $1 in
      --no-quota) quota=0 ;;
      --probe) do_probe=1 ;;
      --json) as_json=1 ;;
      --account=*) only=${1#--account=} ;;
      --account) shift; only=${1:-}; [ -n "$only" ] || die '--account requires a value' ;;
      --timeout=*) timeout=${1#--timeout=} ;;
      --timeout) shift; timeout=${1:-}; [ -n "$timeout" ] || die '--timeout requires a value' ;;
      -h|--help) list_usage; return 0 ;;
      *) list_usage >&2; die "list: unknown option '$1'" ;;
    esac
    shift
  done
  case $timeout in ''|*[!0-9]*) die 'list: --timeout wants whole seconds' ;; esac
  [ "$timeout" -gt 0 ] || die 'list: --timeout wants at least 1 second'
  [ "$quota" = 1 ] || [ "$do_probe" = 0 ] || die 'list: --no-quota and --probe contradict each other'

  labels=$(registry_labels)
  if [ -n "$only" ]; then
    registry_has "$only" || die "unknown account '$only': $(known_accounts_msg)"
    labels=$only
  fi
  if [ -z "$labels" ]; then
    if [ "$as_json" = 1 ]; then printf '[]\n'
    else printf 'No accounts configured. Add one with: %s config add <label>\n' "$PROG"; fi
    return 0
  fi
  def=$(registry_default)

  if [ "$quota" = 1 ]; then
    unset ANTHROPIC_API_KEY ANTHROPIC_AUTH_TOKEN 2>/dev/null || true
    command -v curl >/dev/null 2>&1 || die 'curl is not on PATH (use --no-quota)'
    command -v "$PYTHON" >/dev/null 2>&1 || die "$PYTHON is not on PATH (use --no-quota)"
    WORK=$(mktemp -d "${TMPDIR:-/tmp}/claude-proxy.XXXXXX")
    trap 'rm -rf "$WORK"' EXIT INT TERM
  fi

  if [ "$as_json" = 0 ]; then
    if [ "$quota" = 1 ]; then
      printf '%-14s %-4s %-9s %-9s %-6s %-24s %-6s %-24s %s\n' \
        LABEL DEF KEYCHAIN SOURCE 5H '5H RESETS' 7D '7D RESETS' OTHER
    else
      printf '%-14s %-4s %-9s %s\n' LABEL DEF KEYCHAIN SERVICE
    fi
  fi

  rows=''
  for l in $labels; do
    svc=$(registry_service "$l")
    if "$SECURITY" find-generic-password -a "$KC_ACCOUNT" -s "$svc" >/dev/null 2>&1; then
      kc=present
    else
      kc=MISSING
    fi
    [ "$l" = "$def" ] && mark='*' || mark='-'

    P_ERR=''; P_5=''; P_5R=''; P_7=''; P_7R=''; P_TAIL=''; P_BODY=''
    P_SOURCE=''; P_NOKEY=''
    [ "$quota" = 1 ] && { read_endpoint "$l" "$timeout" || true; }
    # Probe only what the endpoint could not answer for free, and only when
    # the caller asked for it: each one costs a request.
    if [ "$do_probe" = 1 ] && [ -n "$P_ERR" ] && [ "$P_NOKEY" != 1 ]; then
      probe_claude "$l" "$timeout" || true
    fi

    if [ "$as_json" = 1 ]; then
      row="  {\"label\": \"$(jesc "$l")\", \"default\": $([ "$l" = "$def" ] && printf true || printf false), \"keychain\": \"$kc\""
      [ -n "$P_SOURCE" ] && row="$row, \"source\": \"$P_SOURCE\""
      [ -n "$P_ERR" ] && row="$row, \"error\": \"$(jesc "$P_ERR")\""
      if [ -n "$P_BODY" ]; then
        raw=$("$PYTHON" -c 'import json,sys; print(json.dumps(json.load(open(sys.argv[1])), separators=(", ", ": ")))' "$P_BODY" 2>/dev/null || true)
        [ -n "$raw" ] && row="$row, \"usage\": $raw"
      fi
      row="$row}"
      if [ -n "$rows" ]; then rows="$rows,
$row"; else rows=$row; fi
      continue
    fi

    if [ "$quota" = 0 ]; then
      printf '%-14s %-4s %-9s %s\n' "$l" "$mark" "$kc" "$svc"
    elif [ -n "$P_ERR" ]; then
      printf '%-14s %-4s %-9s %-9s %s\n' "$l" "$mark" "$kc" '-' "$P_ERR"
    else
      printf '%-14s %-4s %-9s %-9s %-6s %-24s %-6s %-24s %s\n' \
        "$l" "$mark" "$kc" "$P_SOURCE" \
        "$(fmt_pct "$P_5")" "$(when "$P_5R")" \
        "$(fmt_pct "$P_7")" "$(when "$P_7R")" "${P_TAIL:--}"
    fi
  done

  [ "$as_json" = 1 ] && printf '[\n%s\n]\n' "$rows"
  return 0
}

case ${1:-} in
  help|--help|-h)
    usage; exit 0 ;;
  list)
    shift
    cmd_list_rich "$@"
    exit $? ;;
  config)
    shift
    sub=${1:-}
    [ -n "$sub" ] && shift
    case $sub in
      list) cmd_list ;;
      add) cmd_add "${1:-}" ;;
      rm) cmd_rm "${1:-}" "${2:-}" ;;
      default) cmd_default "${1:-}" ;;
      *) usage >&2; die "unknown config command '${sub:-}'" ;;
    esac
    exit 0 ;;
esac

# --account may sit anywhere; it and its value are consumed, the rest keeps its
# original order. Rotating the positional parameters is the POSIX way to do
# that without arrays.
account=''
n=$#
while [ "$n" -gt 0 ]; do
  arg=$1; shift; n=$((n - 1))
  case $arg in
    --account=*) account=${arg#--account=} ;;
    --account)
      [ "$n" -gt 0 ] || die '--account requires a value'
      account=$1; shift; n=$((n - 1)) ;;
    --)
      set -- "$@" "$arg"
      while [ "$n" -gt 0 ]; do set -- "$@" "$1"; shift; n=$((n - 1)); done ;;
    *) set -- "$@" "$arg" ;;
  esac
done

[ -n "$account" ] || account=${CLAUDE_PROXY_ACCOUNT:-}
[ -n "$account" ] || account=$(registry_default)
[ -n "$account" ] || die "no account given and no default set. $(known_accounts_msg)"
registry_has "$account" || die "unknown account '$account': $(known_accounts_msg)"

command -v claude >/dev/null 2>&1 || die 'claude is not on PATH'

svc=$(registry_service "$account")
TOKEN=$("$SECURITY" find-generic-password -a "$KC_ACCOUNT" -s "$svc" -w 2>/dev/null || true)
if [ -z "$TOKEN" ]; then
  printf '%s: no token in the Keychain for account '\''%s'\'' (service %s).\n' "$PROG" "$account" "$svc" >&2
  printf "  Add it again with:  %s config rm %s && %s config add %s\n" "$PROG" "$account" "$PROG" "$account" >&2
  exit 1
fi

# A stray ANTHROPIC_API_KEY would take precedence and silently bill the wrong
# account. Drop both for this process.
unset ANTHROPIC_API_KEY ANTHROPIC_AUTH_TOKEN 2>/dev/null || true

CLAUDE_CODE_OAUTH_TOKEN=$TOKEN
export CLAUDE_CODE_OAUTH_TOKEN
TOKEN=''
unset TOKEN

exec claude "$@"

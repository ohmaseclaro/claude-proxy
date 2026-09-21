#!/bin/sh
# No real Keychain, no real `claude`, no real config dir: everything is
# redirected at the two override variables plus PATH.
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
TMP=$(mktemp -d "${TMPDIR:-/tmp}/claude-proxy-tests.XXXXXX")
trap 'rm -rf "$TMP"' EXIT INT TERM

export CLAUDE_PROXY_CONFIG_DIR="$TMP/config"
export CLAUDE_PROXY_SECURITY_BIN="$ROOT/test/fake-security"
export FAKE_KEYCHAIN_DIR="$TMP/keychain"
mkdir -p "$TMP/bin"
cp "$ROOT/test/fake-claude" "$TMP/bin/claude"
# Deliberately NOT $PATH: the real `claude` must be unreachable from the suite.
PATH="$TMP/bin:/usr/bin:/bin"; export PATH
REGISTRY="$CLAUDE_PROXY_CONFIG_DIR/accounts.json"
unset CLAUDE_PROXY_ACCOUNT

pass=0; fail=0
ok()  { pass=$((pass + 1)); printf '  ok    %s\n' "$1"; }
bad() { fail=$((fail + 1)); printf '  FAIL  %s\n        %s\n' "$1" "$2"; }

run() { out=$("$ROOT/claude-proxy" "$@" 2>&1); st=$?; }
run_in() { _in=$1; shift; out=$(printf '%s\n' "$_in" | "$ROOT/claude-proxy" "$@" 2>&1); st=$?; }

has()    { case $2 in *"$1"*) ok "$3" ;; *) bad "$3" "missing '$1' in: $out" ;; esac; }
hasnt()  { case $2 in *"$1"*) bad "$3" "found '$1' in: $out" ;; *) ok "$3" ;; esac; }
status() { if [ "$st" = "$1" ]; then ok "$2"; else bad "$2" "exit $st, wanted $1; output: $out"; fi; }

echo "claude-proxy tests"

# --- label validation -------------------------------------------------------
for label in Work 'bad label' -lead 'semi;colon' ''; do
  run_in tok config add "$label"
  if [ "$st" != 0 ]; then ok "rejects invalid label '$label'"
  else bad "rejects invalid label '$label'" "exit 0: $out"; fi
done

# --- add --------------------------------------------------------------------
run_in tok-work config add work
status 0 'config add work succeeds'
has 'claude-proxy-work' "$out" 'config add reports the keychain service'
[ -f "$FAKE_KEYCHAIN_DIR/${USER}.claude-proxy-work" ] \
  && ok 'config add wrote the keychain item' || bad 'config add wrote the keychain item' 'no item file'
[ "$(cat "$FAKE_KEYCHAIN_DIR/${USER}.claude-proxy-work")" = 'tok-work' ] \
  && ok 'keychain item holds the token verbatim' || bad 'keychain item holds the token verbatim' 'mismatch'

run_in tok-perso config add personal
status 0 'config add personal succeeds'
run_in tok-dup config add work
status 1 'config add refuses a duplicate label'

# --- registry file ----------------------------------------------------------
perm=$(ls -l "$REGISTRY" | cut -c1-10)
[ "$perm" = '-rw-------' ] && ok 'registry is chmod 600' || bad 'registry is chmod 600' "perms $perm"
reg=$(cat "$REGISTRY")
hasnt 'tok-work' "$reg" 'registry holds no token'
hasnt 'tok-perso' "$reg" 'registry holds no second token'
has '"default": "work"' "$reg" 'first account became the default'
has '"service": "claude-proxy-personal"' "$reg" 'registry records the service name'

# --- list -------------------------------------------------------------------
run config list
status 0 'config list succeeds'
has 'work' "$out" 'config list shows work'
has 'personal' "$out" 'config list shows personal'
has 'present' "$out" 'config list reports keychain presence'
hasnt 'tok-work' "$out" 'config list never prints a token'
hasnt 'tok-perso' "$out" 'config list never prints the other token'

# --- invocation -------------------------------------------------------------
run --account=work -p 'hello world' --model opus
status 0 '--account=label runs'
has 'claude-args: -p hello world --model opus' "$out" '--account=label is consumed, rest passed verbatim'
has 'oauth-token: tok-work' "$out" 'token reaches claude in the environment'
has 'claude-argc: 4' "$out" 'quoted arguments keep their boundaries'

run --account personal -p hi
has 'claude-args: -p hi' "$out" '--account label (space form) is consumed'
has 'oauth-token: tok-perso' "$out" 'space form picks the right token'

run -p first --account work --model sonnet -- --account inner
has 'claude-args: -p first --model sonnet -- --account inner' "$out" \
  'flag anywhere; order preserved; args after -- untouched'

ANTHROPIC_API_KEY=leak ANTHROPIC_AUTH_TOKEN=leak2 run --account=work -p x
has 'api-key: <unset>' "$out" 'ANTHROPIC_API_KEY is unset before exec'
has 'auth-token: <unset>' "$out" 'ANTHROPIC_AUTH_TOKEN is unset before exec'

run -p 'no flag'
has 'oauth-token: tok-work' "$out" 'falls back to the default account'

CLAUDE_PROXY_ACCOUNT=personal run -p x
has 'oauth-token: tok-perso' "$out" 'CLAUDE_PROXY_ACCOUNT is honoured'
CLAUDE_PROXY_ACCOUNT=personal run --account=work -p x
has 'oauth-token: tok-work' "$out" '--account beats CLAUDE_PROXY_ACCOUNT'

run --account nope -p x
status 1 'unknown account exits non-zero'
has 'known accounts: work personal' "$out" 'unknown account lists the known ones'

run --account
status 1 '--account without a value exits non-zero'

# --- missing token ----------------------------------------------------------
mv "$FAKE_KEYCHAIN_DIR/${USER}.claude-proxy-personal" "$TMP/stashed"
run --account=personal -p x
status 1 'missing keychain item exits non-zero'
has 'no token in the Keychain' "$out" 'missing keychain item is explained'
mv "$TMP/stashed" "$FAKE_KEYCHAIN_DIR/${USER}.claude-proxy-personal"

# --- claude missing ---------------------------------------------------------
mv "$TMP/bin/claude" "$TMP/claude-hidden"
run --account=work -p x
status 1 'missing claude binary exits non-zero'
has 'not on PATH' "$out" 'missing claude binary is explained'
mv "$TMP/claude-hidden" "$TMP/bin/claude"

# --- default ----------------------------------------------------------------
run config default personal
status 0 'config default succeeds'
run -p x
has 'oauth-token: tok-perso' "$out" 'new default takes effect'
run config default nope
status 1 'config default rejects an unknown label'

# --- rm ---------------------------------------------------------------------
run_in n config rm work
status 1 'config rm aborts on a "no" answer'
[ -f "$FAKE_KEYCHAIN_DIR/${USER}.claude-proxy-work" ] \
  && ok 'aborted rm keeps the keychain item' || bad 'aborted rm keeps the keychain item' 'item gone'

run_in y config rm work
status 0 'config rm removes on a "yes" answer'
[ -f "$FAKE_KEYCHAIN_DIR/${USER}.claude-proxy-work" ] \
  && bad 'config rm deletes the keychain item' 'item still there' || ok 'config rm deletes the keychain item'
hasnt '"label": "work"' "$(cat "$REGISTRY")" 'config rm deletes the registry entry'
run --account=work -p x
status 1 'a removed account can no longer be used'

run config rm personal -y
status 0 'config rm -y skips the prompt'
has '"default": null' "$(cat "$REGISTRY")" 'removing the default clears it'
run -p x
status 1 'no account and no default exits non-zero'
has 'no accounts configured' "$out" 'empty registry says how to add one'

printf '\n%s passed, %s failed\n' "$pass" "$fail"
[ "$fail" = 0 ]

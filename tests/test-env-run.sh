#!/usr/bin/env bash
# Exercise the actual recipe body with the real dotenv normalizer and a fake
# SOPS process. No live key, ciphertext, provider call, or environment dump.
set +x
set -euo pipefail
source_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
with_just=0
case "${1:-}" in '') ;; --with-just) with_just=1 ;; *) exit 2 ;; esac
umask 077
tmp=$(mktemp -d)
trap 'rm -rf -- "$tmp"' EXIT
trap 'exit 130' HUP INT TERM
export HOME="$tmp/home" GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null
export CHILD="$tmp/child.json" SOPS_CALLED="$tmp/sops-called" INJECTION="$tmp/injected"
mkdir -p "$HOME" "$tmp/bin" "$tmp/repo with spaces/.just" "$tmp/repo with spaces/env/enc"
repo="$tmp/repo with spaces"
cp "$source_root/.just/env.just" "$source_root/.just/dotenv.py" "$repo/.just/"
cat > "$repo/justfile" <<'JUST'
set shell := ["bash", "-euo", "pipefail", "-c"]
set dotenv-load := false
import '.just/env.just'
JUST
python3 - "$repo/.just/env.just" "$tmp/recipe.sh" <<'PY'
import pathlib, sys, textwrap
source = pathlib.Path(sys.argv[1]).read_text()
header = 'env-run name +cmd: _env-dec\n'
assert source.count(header) == 1
body = source.split(header, 1)[1].split('\n# Show which variables differ', 1)[0]
assert '{{' not in body, 'env-run must not interpolate profile/command source'
pathlib.Path(sys.argv[2]).write_text(textwrap.dedent(body))
PY
bash -n "$tmp/recipe.sh"
cat > "$tmp/bin/sops" <<'STUB'
#!/usr/bin/env bash
printf 'called\n' >> "$SOPS_CALLED"
case "${TEST_MODE:-ok}" in
  fail) printf '%s\n' 'EXAMPLE="partial"'; echo 'DO_NOT_LOG_PROVIDER_DETAIL' >&2; exit 23 ;;
  parse-fail) printf '%s\n' 'EXAMPLE="\uZZZZ"' ;;
  *) printf '%s\n' 'EXAMPLE="hello world"' ;;
esac
STUB
cat > "$tmp/bin/ores-sops" <<'STUB'
#!/usr/bin/env bash
[ "$1" = ensure-dec ] || exit 2
umask 077
mkdir -p env/dec
STUB
chmod +x "$tmp/bin/sops" "$tmp/bin/ores-sops"
export PATH="$tmp/bin:$PATH"
cd "$repo"
git init -q --template=
printf 'runtime-only ciphertext placeholder for the stub\n' > env/enc/dev.env.enc
runner=(bash "$tmp/recipe.sh")
if [ "$with_just" -eq 1 ]; then command -v just >/dev/null; runner=(just env-run); fi
child='import json,os,pathlib,sys; pathlib.Path(os.environ["CHILD"]).write_text(json.dumps({"args":sys.argv[1:],"value":os.environ.get("EXAMPLE"),"scratch":os.environ.get("_ores_sops_exports")}))'

env _ores_sops_exports=ambient "${runner[@]}" dev python3 -c "$child" 'two words' "quote'd" 'literal$(echo no)' > "$tmp/output" 2>&1
python3 - "$CHILD" <<'PY'
import json, pathlib, sys
value = json.loads(pathlib.Path(sys.argv[1]).read_text())
assert value == {'args': ['two words', "quote'd", 'literal$(echo no)'], 'value': 'hello world', 'scratch': None}
PY
printf 'ok - success preserves normalized values and exact argv without exporting scratch text\n'

for mode in fail parse-fail; do
  rm -f -- "$CHILD"
  if env TEST_MODE="$mode" "${runner[@]}" dev python3 -c "$child" > "$tmp/output" 2>&1; then
    echo 'unexpected success after decrypt/normalizer failure' >&2; exit 1
  fi
  [ ! -e "$CHILD" ]
  ! grep -qE 'DO_NOT_LOG_PROVIDER_DETAIL|partial|uZZZZ' "$tmp/output"
  printf 'ok - decrypt/normalizer failure never starts the child or prints source diagnostics\n'
done

for profile in staging '../escape' 'dev; touch "$INJECTION"' "dev'"; do
  : > "$SOPS_CALLED"
  if "${runner[@]}" "$profile" python3 -c "$child" > "$tmp/output" 2>&1; then exit 1; fi
  [ ! -e "$CHILD" ] && [ ! -e "$INJECTION" ] && [ ! -s "$SOPS_CALLED" ]
  printf 'ok - hostile or unsupported profile is rejected before decryption\n'
done
rm env/enc/dev.env.enc
printf 'keep\n' > "$tmp/outside"
ln -s "$tmp/outside" env/enc/dev.env.enc
: > "$SOPS_CALLED"
if "${runner[@]}" dev python3 -c "$child" > "$tmp/output" 2>&1; then exit 1; fi
[ ! -e "$CHILD" ] && [ ! -s "$SOPS_CALLED" ]
[ "$(cat "$tmp/outside")" = keep ]
printf 'ok - ciphertext symlink is rejected without changing its target\n'
printf 'PASS: 8 fail-closed runner checks (real Just=%s; SOPS stubbed)\n' "$with_just"

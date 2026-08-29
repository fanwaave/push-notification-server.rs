#!/usr/bin/env bash
set -euo pipefail

repo_root=$(git rev-parse --show-toplevel)
cd "$repo_root"

fail() {
  echo "encrypted-env policy: $*" >&2
  exit 1
}

for file in .gitignore .gitattributes .sops.yaml .env.example justfile env/README.md scripts/verify-sops-release-policy.py; do
  test -f "$file" || fail "missing $file"
done

git check-ignore --no-index -q .env || fail "root .env must be ignored"
git check-ignore --no-index -q sample.env || fail "root plaintext dotenv must be ignored"
git check-ignore --no-index -q sample.env.local || fail "suffixed dotenv must be ignored"
git check-ignore --no-index -q nested/sample.env || fail "nested dotenv must be ignored"
git check-ignore --no-index -q nested/deeper/sample.env.local || fail "deep dotenv must be ignored"
git check-ignore --no-index -q env/dec/dev.env || fail "env/dec plaintext must be ignored"
if git check-ignore --no-index -q env/enc/dev.env.enc; then
  fail "approved dev ciphertext is ignored"
fi
if git check-ignore --no-index -q env/enc/prod.env.enc; then
  fail "approved prod ciphertext is ignored"
fi

grep -Fq '/env/enc/*.env.enc text eol=lf' .gitattributes \
  || fail "missing ciphertext LF normalization"
grep -Fq 'path_regex: ^env/enc/dev\.env\.enc$' .sops.yaml \
  || fail "missing exact dev SOPS creation rule"
grep -Fq 'path_regex: ^env/enc/prod\.env\.enc$' .sops.yaml \
  || fail "missing exact prod SOPS creation rule"

rule_count=$(grep -E '^[[:space:]-]*path_regex: .*env/enc' .sops.yaml | wc -l | tr -d ' ')
test "$rule_count" = 2 || fail "only exact dev/prod env/enc rules are allowed"
recipient_count=$(grep -Eo 'age1[a-z0-9]{58}' .sops.yaml | sort -u | wc -l | tr -d ' ')
test "$recipient_count" -ge 3 || fail "dev/prod policy requires at least three distinct public recipients"
python3 scripts/verify-sops-release-policy.py .sops.yaml prod

python3 - <<'PY'
from pathlib import Path
text = Path("justfile").read_text(encoding="utf-8")
if "mkdir -p env/dec" in text or "mkdir -p env/enc env/dec" in text:
    raise SystemExit("justfile must not mkdir env/dec; ores-sops ensure-dec owns that path")
if "chmod 700 env/dec" in text:
    raise SystemExit("justfile must not chmod env/dec before ores-sops")
PY


is_plaintext_env_path() {
  case "$1" in
    .env.example|*/.env.example) return 1 ;;
    .env|*.env|.env.*|*.env.*|env/dec/*) return 0 ;;
    *) return 1 ;;
  esac
}

while IFS= read -r -d '' path; do
  mode=$(git ls-files -s -- "$path" | awk 'NR==1 { print $1 }')
  case "$path" in
    env/enc/dev.env.enc|env/enc/prod.env.enc)
      test "$mode" != 120000 || fail "approved ciphertext path is a symlink: $path"
      ;;
    env/enc/*)
      fail "unexpected tracked path under env/enc: $path"
      ;;
    .sops.yaml|.gitattributes|.gitignore|.env.example)
      test "$mode" != 120000 || fail "policy path is a symlink: $path"
      ;;
    *)
      if is_plaintext_env_path "$path"; then
        fail "tracked plaintext dotenv path: $path"
      fi
      ;;
  esac
done < <(git ls-files -z)

age_private='AGE-SE''CRET-KEY-1'
pem_private='-----BEGIN ''PRIVATE KEY-----'
openssh_private='-----BEGIN OPENSSH ''PRIVATE KEY-----'
if git grep -I -q -e "$age_private" -e "$pem_private" -e "$openssh_private" -- .; then
  fail "tracked private-key material detected"
fi

for file in env/enc/dev.env.enc env/enc/prod.env.enc; do
  test -f "$file" || continue
  grep -q '^sops_mac=ENC\[' "$file" || fail "$file does not look like SOPS dotenv ciphertext"
  while IFS= read -r line || test -n "$line"; do
    case "$line" in
      sops_*=*) ;;
      [A-Za-z_][A-Za-z0-9_]*=ENC\[*\]) ;;
      [A-Za-z_][A-Za-z0-9_]*=*) fail "$file contains an obvious plaintext assignment" ;;
    esac
  done < "$file"
done

if test -e .env || test -L .env; then
  test -L .env || fail ".env exists but is not a managed symlink"
  target=$(readlink .env)
  case "$target" in
    env/dec/dev.env|env/dec/prod.env) ;;
    *) fail ".env points outside managed env/dec targets" ;;
  esac
fi

if test -d env/dec; then
  mode=$(stat -c '%a' env/dec 2>/dev/null || stat -f '%Lp' env/dec)
  test "$mode" = 700 || fail "env/dec must be mode 0700"
fi

if command -v ores-sops >/dev/null 2>&1; then
  ores-sops verify
fi

echo "encrypted environment policy is valid"

#!/usr/bin/env bash
set -euo pipefail

# Static contract for the security-sensitive live-smoke wiring. The real
# lifecycle remains the acceptance test; these guards prevent a later workflow
# edit from silently restoring readiness-only or literal-secret configuration.
# trace:TASK-1425 trace:ADR-54 | ai:codex
workflow=".github/workflows/cross-platform.yml"

grep -Fq 'GLAB_VERSION: 1.118.0' "$workflow"
grep -Fq '"https://${AIDA_GITLAB_LIVE_HOST}/api/v4/version"' "$workflow"
grep -Fq '"https://${AIDA_GITLAB_LIVE_HOST}/cdn-cgi/access/get-identity"' "$workflow"
grep -Fq "jq -e '.version | type == \"string\" and length > 0'" "$workflow"
grep -Fq 'body_class="cloudflare-access-html"' "$workflow"
grep -Fq 'body_class="cloudflare-access-login-redirect"' "$workflow"
grep -Fq 'location_path=${location:-none}' "$workflow"
grep -Fq 'valueFromEnv: AIDA_CF_ACCESS_CLIENT_ID' "$workflow"
grep -Fq 'valueFromEnv: AIDA_CF_ACCESS_CLIENT_SECRET' "$workflow"
grep -Fq 'chmod 600 "$GLAB_CONFIG_DIR/config.yml"' "$workflow"
grep -Fq 'export GIT_CONFIG_COUNT=3' "$workflow"
grep -Fq 'http.https://${AIDA_GITLAB_LIVE_HOST}/.extraHeader' "$workflow"
grep -Fq 'credential.https://${AIDA_GITLAB_LIVE_HOST}.helper' "$workflow"
grep -Fq "export GIT_CONFIG_VALUE_2='!glab auth git-credential'" "$workflow"
grep -Fq "trap 'unset GIT_CONFIG_COUNT" "$workflow"

if grep -Eq 'git config (--global|--local).*CF-Access' "$workflow"; then
  echo "Access secrets must not be persisted in Git configuration" >&2
  exit 1
fi

if grep -Fq '/-/readiness' "$workflow"; then
  echo "live GitLab preflight must not use readiness-only evidence" >&2
  exit 1
fi

for secret in AIDA_GITLAB_TOKEN AIDA_CF_ACCESS_CLIENT_ID AIDA_CF_ACCESS_CLIENT_SECRET; do
  grep -Fq '${{ secrets.'"$secret"' }}' "$workflow"
done

# Prove Git accepts duplicate, host-scoped extraHeader entries through numbered
# environment config without creating a persistent user config.
test_home="$(mktemp -d)"
trap 'rm -rf "$test_home"' EXIT
header_count="$(
  HOME="$test_home" \
  GIT_CONFIG_COUNT=2 \
  GIT_CONFIG_KEY_0='http.https://gitlab.example.test/.extraHeader' \
  GIT_CONFIG_VALUE_0='CF-Access-Client-Id: dummy-id' \
  GIT_CONFIG_KEY_1='http.https://gitlab.example.test/.extraHeader' \
  GIT_CONFIG_VALUE_1='CF-Access-Client-Secret: dummy-secret' \
  git config --get-all 'http.https://gitlab.example.test/.extraHeader' | wc -l
)"
test "$header_count" -eq 2
test ! -e "$test_home/.gitconfig"

echo "PASS: GitLab live smoke requires Access-authenticated JSON and env-backed glab headers"

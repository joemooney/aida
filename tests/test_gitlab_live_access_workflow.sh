#!/usr/bin/env bash
set -euo pipefail

# Static contract for the security-sensitive live-smoke wiring. The real
# lifecycle remains the acceptance test; these guards prevent a later workflow
# edit from silently restoring readiness-only or literal-secret configuration.
# trace:TASK-1425 trace:ADR-54 | ai:codex
workflow=".github/workflows/cross-platform.yml"

grep -Fq 'GLAB_VERSION: 1.118.0' "$workflow"
grep -Fq '"https://${AIDA_GITLAB_LIVE_HOST}/api/v4/version"' "$workflow"
grep -Fq "jq -e '.version | type == \"string\" and length > 0'" "$workflow"
grep -Fq 'if [ "$curl_status" -eq 22 ]' "$workflow"
grep -Fq 'valueFromEnv: AIDA_CF_ACCESS_CLIENT_ID' "$workflow"
grep -Fq 'valueFromEnv: AIDA_CF_ACCESS_CLIENT_SECRET' "$workflow"
grep -Fq 'chmod 600 "$GLAB_CONFIG_DIR/config.yml"' "$workflow"

if grep -Fq '/-/readiness' "$workflow"; then
  echo "live GitLab preflight must not use readiness-only evidence" >&2
  exit 1
fi

for secret in AIDA_GITLAB_TOKEN AIDA_CF_ACCESS_CLIENT_ID AIDA_CF_ACCESS_CLIENT_SECRET; do
  grep -Fq '${{ secrets.'"$secret"' }}' "$workflow"
done

echo "PASS: GitLab live smoke requires Access-authenticated JSON and env-backed glab headers"

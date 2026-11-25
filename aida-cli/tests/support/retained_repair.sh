#!/usr/bin/env bash
# A staged punt survives preview, a moved base and a same-base successor.
# trace:BUG-2761.AC1 BUG-2761.AC2 | ai:codex
set -euo pipefail
cli=$1 spec=$2 branch=${2,,}
root=$(dirname "$PWD")
origin=$root/origin.git
export CODEX_HOME=$HOME/.codex AIDA_AGENT_CMD=$root/bin/codex AIDA_TEST_GH_BINARY=$root/bin/gh
rm "$root/bin/bwrap" # use the real host repair sandbox
mkdir -p "$CODEX_HOME"
printf '%s\n' "sandbox_mode='workspace-write'" "approval_policy='never'" '[sandbox_workspace_write]' 'network_access=true' 'writable_roots=[]' > "$CODEX_HOME/config.toml"
printf '%s\n' "store_path='.aida-store'" '[agents]' "vendor='codex'" "enabled=['codex']" '[contained]' 'enable=true' "seat_tree='clone'" '[contained.vendors]' "codex='native'" '[doctor.disk_headroom]' 'min_free_gib=1' '[forge]' 'provider="github"' '[validation]' "fast_check='true'" "repair_check='true'" > .aida/config.toml
echo seed > conflict
git add -f .aida/config.toml conflict
git commit -qm policy
git clone --bare . "$origin"
git remote add origin "$origin"
git checkout -qb "$branch"
echo feature > conflict
git commit -qam "feature (Refs $spec)"
git push origin "$branch"
head=$(git rev-parse HEAD)
git checkout -q main
echo main > conflict
git commit -qam conflict
git push origin main
git remote set-head origin main
base=$(git rev-parse HEAD)
cat > "$AIDA_TEST_GH_BINARY" <<GH
#!/bin/sh
set -eu
tip=\$(git --git-dir='$origin' rev-parse 'refs/heads/$branch')
pr='{"number":7,"state":"OPEN","isCrossRepository":false,"body":"","headRefOid":"'\$tip'","baseRefName":"main","headRefName":"$branch","title":"repair ($spec)","url":"https://github.com/example/test/pull/7"}'
case "\$*" in
  *'--state merged'*) data='[]' ;; *'pr list'*) data="[\$pr]" ;;
  *'pr view'*) data="\$pr" ;; *'pr edit'*) exit 0 ;; *) exit 1 ;;
esac
while [ "\$#" -gt 1 ]; do
  case "\$1" in -q|--jq) echo "\$data" | jq -r "\$2"; exit ;; esac
  shift
done
echo "\$data"
GH
cat > "$AIDA_AGENT_CMD" <<VENDOR
#!/bin/sh
set -eu
case " \$* " in
  *" sandbox "*) while [ "\$1" != -- ]; do shift; done; shift; exec "\$@" ;;
  *" --version "*) echo 'codex fixture'; exit ;; *"FIRST TASK"*) ;; *) exit 81 ;;
esac
if [ ! -f '$root/first-seat' ]; then
  pwd -P > '$root/first-seat'
  echo 'staged resolution' > conflict
  git add -- conflict
  exec '$cli' punt '$spec' --category design-fork --reason 'review resolution'
fi
test "\$(pwd -P)" = "\$(cat '$root/first-seat')"
test "\$(git show :conflict)" = 'staged resolution'
exec '$cli' ship -m '[AI:codex] fix: repair (Refs $spec)'
VENDOR
chmod +x "$AIDA_AGENT_CMD"
"$cli" queue add "$spec" --for implementer
status=0
"$cli" queue work "$spec" --role implementer --no-human --no-pull || status=$?
test "$status" = 32
"$cli" queue rework "$spec" --reason 'reviewed resolution'
seat=$(cat "$root/first-seat")
journal=.aida/repair-journal.json
previous=$(jq -er '.entries[0].lane_id' "$journal")
claims() { git -C .aida-store ls-files -s 'coordination/leases/*.toml'; }
prior_claims=$(claims)
echo 'operator edit' > "$seat/conflict"
sha256sum "$journal" ".aida/sessions/$previous.toml" "$seat"/.git/{index,MERGE_HEAD,MERGE_MSG} "$seat/conflict" > "$root/before"
for mode in --no-launch --plan-only --guided; do
  "$cli" queue work "$spec" --role implementer "$mode" | grep -F 'retained repair available'
  sha256sum --check "$root/before"
done
retry() { "$cli" queue rework "$spec" --work --no-pull --reason 'resume repair'; }
# A resolved path is recoverable only while the index still records its origin.
cp "$seat/.git/index" "$root/staged-index"
git -C "$seat" update-index --clear-resolve-undo
if retry > "$root/refusal" 2>&1; then exit 1; fi
grep -F 'repair conflict paths changed' "$root/refusal"
cp "$root/staged-index" "$seat/.git/index"
sha256sum --check "$root/before"
git commit --allow-empty -qm 'unrelated advance'
git push origin main
if retry > "$root/refusal" 2>&1; then exit 1; fi
cat "$root/refusal"
grep -F 'base moved: needs a fresh repair round' "$root/refusal"
sha256sum --check "$root/before"
test "$(claims)" = "$prior_claims"
test "$(git --git-dir="$origin" rev-parse "$branch")" = "$head"
# Restore the fixture origin to the original base for the successful dispatch.
git push --force-with-lease origin "$base:refs/heads/main"
echo 'staged resolution' > "$seat/conflict"
retry
test "$(git --git-dir="$origin" show -s --format=%P "$branch")" = "$head $base"
test "$(git --git-dir="$origin" show "$branch:conflict")" = 'staged resolution'

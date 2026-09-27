#!/usr/bin/env bash
# trace:TASK-1520 | ai:codex
set -euo pipefail

ROOT="${AIDA_REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
TEMPLATES="$ROOT/aida-core/templates"
INVENTORY="$TEMPLATES/skill-inventory.toml"
SKILLS_ROOT="$ROOT/.agents/skills"
MODE="${1:-}"

if [[ "$MODE" != sync && "$MODE" != check && "$MODE" != list ]]; then
  echo "Usage: $0 {sync|check|list}" >&2
  exit 2
fi

mapfile -t EXCLUDED < <(awk '
  /^\[\[(claude_only|not_a_skill)\]\]/ { excluded=1; next }
  /^\[\[/ { excluded=0; next }
  excluded && /^name = / { gsub(/"/, "", $3); print $3 }
' "$INVENTORY" | sort -u)
mapfile -t SKILLS < <(
    find "$TEMPLATES/skills" -mindepth 1 -maxdepth 2 -type f \
    \( -name '*.md' -o -name SKILL.md \) -print |
    while IFS= read -r file; do
      if [[ "$(basename "$file")" == SKILL.md ]]; then
        name="$(basename "$(dirname "$file")")"
      elif [[ "$(dirname "$file")" == "$TEMPLATES/skills" ]]; then
        name="$(basename "$file" .md)"
      else
        continue
      fi
      [[ "$name" == *.local ]] && continue
      skip=0
      for excluded in "${EXCLUDED[@]}"; do [[ "$name" == "$excluded" ]] && skip=1; done
      [[ $skip -eq 0 ]] && printf '%s\n' "$name"
    done | sort -u
)

source_for() {
  local name="$1"
  if [[ -f "$TEMPLATES/skills-portable/$name.md" ]]; then
    printf '%s\n' "$TEMPLATES/skills-portable/$name.md"
  elif [[ -f "$TEMPLATES/skills/$name.md" ]]; then
    printf '%s\n' "$TEMPLATES/skills/$name.md"
  else
    printf '%s\n' "$TEMPLATES/skills/$name/SKILL.md"
  fi
}

if [[ "$MODE" == list ]]; then
  printf '%s\n' "${SKILLS[@]}"
  exit 0
fi

fail=0
if [[ "$MODE" == sync ]]; then
  mkdir -p "$SKILLS_ROOT"
  # AIDA owns only aida-* names in this shared directory.
  while IFS= read -r entry; do
    [[ "$entry" == aida-* ]] || continue
    keep=0
    for name in "${SKILLS[@]}"; do [[ "$entry" == "$name" ]] && keep=1; done
    if [[ $keep -eq 0 ]]; then rm -rf -- "$SKILLS_ROOT/$entry"; fi
  done < <(find "$SKILLS_ROOT" -mindepth 1 -maxdepth 1 -type d -printf '%f\n' | sort)

  for name in "${SKILLS[@]}"; do
    src="$(source_for "$name")"
    target="$SKILLS_ROOT/$name"
    tmp="$SKILLS_ROOT/.${name}.tmp.$$"
    rm -rf -- "$tmp"
    mkdir -p "$tmp"
    if [[ "$src" == */SKILL.md ]]; then
      cp "$src" "$tmp/SKILL.md"
      src_dir="$(dirname "$src")"
      while IFS= read -r -d '' file; do
        rel="${file#"$src_dir/"}"
        [[ "$rel" == SKILL.md ]] && continue
        mkdir -p "$tmp/$(dirname "$rel")"
        cp "$file" "$tmp/$rel"
      done < <(find "$src_dir" -type f -print0)
    else
      cp "$src" "$tmp/SKILL.md"
    fi
    rm -rf -- "$target"
    mv "$tmp" "$target"
    echo "  Copied: .agents/skills/$name/"
  done
else
  [[ -d "$SKILLS_ROOT" ]] || { echo "MISSING: .agents/skills"; exit 1; }
  for name in "${SKILLS[@]}"; do
    src="$(source_for "$name")"
    target="$SKILLS_ROOT/$name"
    if [[ ! -f "$target/SKILL.md" ]]; then
      echo "MISSING: .agents/skills/$name/SKILL.md"; fail=1; continue
    fi
    if [[ -L "$target/SKILL.md" ]]; then
      echo "SYMLINK: .agents/skills/$name/SKILL.md (must be a regular file)"; fail=1; continue
    fi
    if [[ "$src" == */SKILL.md ]]; then
      if ! diff -qr "$(dirname "$src")" "$target" >/dev/null 2>&1; then
        echo "DRIFT: .agents/skills/$name/ differs from $src"; fail=1
      else echo "  OK: .agents/skills/$name/"; fi
    elif ! cmp -s "$src" "$target/SKILL.md"; then
      echo "DRIFT: .agents/skills/$name/SKILL.md differs from $src"; fail=1
    else echo "  OK: .agents/skills/$name/SKILL.md"; fi
  done
  while IFS= read -r entry; do
    [[ "$entry" == aida-* ]] || continue
    found=0
    for name in "${SKILLS[@]}"; do [[ "$entry" == "$name" ]] && found=1; done
    if [[ $found -eq 0 ]]; then echo "STALE: .agents/skills/$entry"; fail=1; fi
  done < <(find "$SKILLS_ROOT" -mindepth 1 -maxdepth 1 -type d -printf '%f\n' | sort)
fi

exit "$fail"

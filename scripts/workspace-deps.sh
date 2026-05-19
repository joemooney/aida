#!/usr/bin/env bash
# scripts/workspace-deps.sh — intra-workspace dependency-pin discovery.
#
# Sourced by scripts/release.sh and tests/test_release_dep_discovery.sh.
# Not meant to be executed directly.
#
# An *intra-workspace dependency pin* is a Cargo dependency line whose
# inline table carries both `path = "../<crate>"` (pointing at another
# workspace member) AND a `version = "X"` constraint. When the workspace
# version is bumped every such pin must move in lockstep, or `cargo` fails
# to resolve the graph:
#
#   error: failed to select a version for the requirement `aida-tui = "^0.7"`
#
# BUG-111: the release script used to bump a *hardcoded* list of crates, so
# a newly-added workspace crate (aida-tui, added by STORY-132) was missed —
# its pin was left stale and `make release-minor` failed. These helpers
# discover the pins generically from `[workspace.members]` — no hardcoded
# list — so a new crate is picked up automatically.
#
# Scope: inline-table dependency declarations (the AIDA convention). A
# multi-line `[dependencies.foo]` table form is not rewritten here; the
# `cargo check --workspace` gate in release.sh is the backstop for anything
# the scanner cannot see.
#
# trace:BUG-111 | ai:claude

# ws_discover_members <root-dir>
#   Echo one workspace-member directory per line, parsed from the
#   [workspace] members array of <root-dir>/Cargo.toml.
ws_discover_members() {
    local root="${1:-.}"
    awk '
        /^\[/ { in_ws = ($0 ~ /^\[workspace\][[:space:]]*$/) }
        in_ws && /^[[:space:]]*members[[:space:]]*=/ { collecting = 1 }
        collecting {
            line = $0
            while (match(line, /"[^"]*"/)) {
                m = substr(line, RSTART + 1, RLENGTH - 2)
                if (m != "") print m
                line = substr(line, RSTART + RLENGTH)
            }
            if ($0 ~ /\]/) collecting = 0
        }
    ' "$root/Cargo.toml"
}

# ws_list_intra_pins <root-dir>
#   Echo one TAB-separated `FILE<TAB>DEP<TAB>VERSION` record per intra-
#   workspace path-dependency pin: a path dep into a workspace member that
#   also carries an explicit `version =` constraint. Scans the workspace
#   root manifest plus every member manifest.
ws_list_intra_pins() {
    local root="${1:-.}"
    local members_basenames m file
    members_basenames=$(ws_discover_members "$root" | awk -F/ '{print $NF}' | tr '\n' ' ')

    local files=("$root/Cargo.toml")
    while IFS= read -r m; do
        [ -n "$m" ] && files+=("$root/$m/Cargo.toml")
    done < <(ws_discover_members "$root")

    for file in "${files[@]}"; do
        [ -f "$file" ] || continue
        awk -v members="$members_basenames" -v file="$file" '
            BEGIN {
                n = split(members, a, " ")
                for (i = 1; i <= n; i++) if (a[i] != "") ws[a[i]] = 1
            }
            /^[[:space:]]*#/ { next }
            {
                line = $0
                # Require a path = "../<crate>" token into a workspace member.
                if (!match(line, /path[[:space:]]*=[[:space:]]*"[^"]*"/)) next
                ptok = substr(line, RSTART, RLENGTH)
                match(ptok, /"[^"]*"/)
                pval = substr(ptok, RSTART + 1, RLENGTH - 2)
                base = pval; sub(/.*\//, "", base)
                if (!(base in ws)) next
                # Require an explicit version constraint to have a pin to bump.
                if (!match(line, /version[[:space:]]*=[[:space:]]*"[^"]*"/)) next
                vtok = substr(line, RSTART, RLENGTH)
                match(vtok, /"[^"]*"/)
                ver = substr(vtok, RSTART + 1, RLENGTH - 2)
                # Dependency key = the text before the first `=`.
                dep = line
                sub(/[[:space:]]*=.*/, "", dep)
                gsub(/[[:space:]]/, "", dep)
                if (dep != "") print file "\t" dep "\t" ver
            }
        ' "$file"
    done
}

# ws_bump_intra_pins <root-dir> <new-version>
#   Rewrite every intra-workspace path-dep version pin to <new-version>.
#   Prints one `  bumped <file>: <dep> <old> -> <new>` line per change.
ws_bump_intra_pins() {
    local root="${1:-.}" newver="$2"
    local file dep ver
    while IFS=$'\t' read -r file dep ver; do
        [ -n "$file" ] || continue
        [ "$ver" = "$newver" ] && continue
        # Targeted: only the line that *starts* the `<dep> = { ... }` inline
        # table; rewrite the first version token on that line.
        sed -i.bak -E \
            "/^${dep}[[:space:]]*=[[:space:]]*\{/ s/version[[:space:]]*=[[:space:]]*\"[^\"]*\"/version = \"${newver}\"/" \
            "$file"
        rm -f "${file}.bak"
        echo "  bumped ${file}: ${dep} ${ver} -> ${newver}"
    done < <(ws_list_intra_pins "$root")
}

# ws_verify_intra_pins <root-dir> <expected-version>
#   Return 0 iff every intra-workspace path-dep pin is at <expected-version>.
#   On a mismatch, print `error: <file>: ...` to stderr for each offender
#   (naming the unbumped crate) and return 1.
ws_verify_intra_pins() {
    local root="${1:-.}" expected="$2"
    local file dep ver bad=0
    while IFS=$'\t' read -r file dep ver; do
        [ -n "$file" ] || continue
        if [ "$ver" != "$expected" ]; then
            echo "error: ${file}: intra-workspace dependency '${dep}' pinned at \"${ver}\", expected \"${expected}\"" >&2
            bad=1
        fi
    done < <(ws_list_intra_pins "$root")
    return "$bad"
}

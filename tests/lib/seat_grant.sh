# STORY-1473 / ADR-66 seat fixture for shell gates.
#
# An env role (`AIDA_SESSION_ROLE`) is only a display/routing hint, so a shell
# gate that exercises an advisor/product-gated CLI step must establish what
# `aida role enter` would at a human TTY: a roster ceiling in the project
# store plus a grant record under the gate's isolated HOME, exported to the
# invoked command via AIDA_SESSION_GRANT. This mirrors
# aida-cli-lib/src/seat_authority.rs::test_support::mint_grant_for exactly; a
# drifting field fails these gates closed, never open. No production bypass.
# trace:STORY-1473 | ai:claude

# aida_seat_grant <project_root> <home> <subject> <seat> [delegable...]
# Writes the roster + grant record and prints the grant id (the value to pass
# as AIDA_SESSION_GRANT). <subject> must match what the invoked binary
# resolves as its user id (AIDA_USER / USER / USERNAME, else "default" —
# `env -i` invocations resolve "default").
aida_seat_grant() {
    local project_root="$1" home="$2" subject="$3" seat="$4"
    shift 4

    mkdir -p "$project_root/.aida"
    local config="$project_root/.aida/config.toml"
    local store="$project_root/.aida-store"
    if [ -f "$config" ]; then
        local configured
        configured=$(sed -n 's/^store_path[[:space:]]*=[[:space:]]*["'\'']\(.*\)["'\'']$/\1/p' "$config" | head -1)
        if [ -n "$configured" ]; then
            store="$project_root/$configured"
        elif ! grep -q '^store_path' "$config"; then
            # Prepend so the key stays top-level (appending after a [table]
            # header would land inside that table).
            printf 'store_path = ".aida-store"\n%s' "$(cat "$config")" >"$config"
        fi
    else
        printf 'store_path = ".aida-store"\n' >"$config"
    fi
    mkdir -p "$store/objects" "$store/registry"

    local seats="\"$seat\"" d
    for d in ${1+"$@"}; do seats="$seats, \"$d\""; done
    printf '[members]\n"%s" = [%s]\n' "$subject" "$seats" \
        >"$store/registry/team.toml"

    local delegable_json="[]"
    if [ "$#" -gt 0 ]; then
        delegable_json=$(printf '"%s", ' "$@")
        delegable_json="[${delegable_json%, }]"
    fi

    local id session now expires
    id=$(cat /proc/sys/kernel/random/uuid 2>/dev/null \
        || python3 -c 'import uuid; print(uuid.uuid4())')
    session=$(cat /proc/sys/kernel/random/uuid 2>/dev/null \
        || python3 -c 'import uuid; print(uuid.uuid4())')
    now=$(date -u +%Y-%m-%dT%H:%M:%SZ)
    expires=$(python3 -c 'import datetime; print((datetime.datetime.now(datetime.timezone.utc) + datetime.timedelta(hours=23)).strftime("%Y-%m-%dT%H:%M:%SZ"))')

    mkdir -p "$home/.aida/session-grants"
    cat >"$home/.aida/session-grants/$id.json" <<EOF
{
  "id": "$id",
  "principal": "$subject",
  "subject": "$subject",
  "session_id": "$session",
  "seat": "$seat",
  "tty_issued_at": "$now",
  "delegable_seats": $delegable_json,
  "parent_grant_id": null,
  "issued_at": "$now",
  "expires_at": "$expires",
  "revoked_at": null
}
EOF
    printf '%s\n' "$id"
}

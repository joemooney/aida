#!/usr/bin/env bash
# trace:BUG-1806 | ai:codex
# Exercise the shipped template with a fake CLI and HOME, never real seat files.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
fixture=$(mktemp -d)
trap 'rm -rf "$fixture"' EXIT
mkdir -p "$fixture/home" "$fixture/bin"
sed -n '/^pub(crate) const SHELL_HELPERS: &str = r#"/,$p' "$root/aida-cli-lib/src/dev_cmd.rs" |
    sed '1s/^[^"]*r#"//; /^"#;$/,$d' > "$fixture/helpers"
cat > "$fixture/bin/aida" <<'STUB'
#!/bin/sh
if [ "${SCENARIO:-}" = refused_stderr ]; then
    cat "$FIXTURE/output" >&2
else
    cat "$FIXTURE/output"
fi
exit "$CLI_RC"
STUB
chmod +x "$fixture/bin/aida"
cat > "$fixture/run" <<'RUN'
. "$FIXTURE/helpers"
export AIDA_SESSION_ROLE=implementer
if aida role enter 'advisor`touch "$HOME/marker"`'; then rc=0; else rc=$?; fi
printf 'rc:%s role:%s\n' "$rc" "$AIDA_SESSION_ROLE"
test ! -e "$HOME/marker"
RUN
for shell in bash zsh; do
    if ! command -v "$shell" >/dev/null; then
        echo "SKIP: $shell unavailable"
        continue
    fi
    # Reproduce the historical unconditional-eval wrapper in isolation. This
    # positive control proves that the refused backtick payload can execute.
    printf '%s\n' 'error: refused advisor request `touch "$HOME/marker"`' > "$fixture/output"
    env -i HOME="$fixture/home" PATH="$fixture/bin:/usr/bin:/bin" FIXTURE="$fixture" CLI_RC=23 \
        "$shell" -f -c 'eval "$(command aida role enter advisor)"; test -e "$HOME/marker"' \
        > "$fixture/control.stdout" 2> "$fixture/control.stderr"
    rm "$fixture/home/marker"
    echo "PASS: $shell historical failure reproduced in fake HOME"
    for scenario in ${*:-refused refused_no_newline refused_stderr refused_block legacy incomplete inline duplicate valid}; do
        cli_rc=0
        expected_role=implementer
        case "$scenario" in
            refused)
                cli_rc=23
                printf '%s\n\n' 'error: refused advisor request `touch "$HOME/marker"` $(touch "$HOME/marker")' > "$fixture/output" ;;
            refused_no_newline|refused_stderr)
                cli_rc=23
                printf '%s' 'error: refused advisor request `touch "$HOME/marker"` $(touch "$HOME/marker")' > "$fixture/output" ;;
            refused_block)
                cli_rc=23
                printf '%s\n' '#aida:eval:begin' 'export AIDA_SESSION_ROLE=advisor' 'touch "$HOME/marker"' '#aida:eval:end' > "$fixture/output" ;;
            legacy)
                printf '%s\n' 'export AIDA_SESSION_ROLE=advisor' 'echo `touch "$HOME/marker"`' > "$fixture/output" ;;
            incomplete)
                printf '%s\n' '#aida:eval:begin' 'export AIDA_SESSION_ROLE=advisor' 'touch "$HOME/marker"' > "$fixture/output" ;;
            inline)
                printf '%s\n' 'prose #aida:eval:begin' 'touch "$HOME/marker"' '#aida:eval:end' > "$fixture/output" ;;
            duplicate)
                printf '%s\n' '#aida:eval:begin' 'touch "$HOME/marker"' '#aida:eval:end' '#aida:eval:begin' '#aida:eval:end' > "$fixture/output" ;;
            valid)
                expected_role=advisor
                printf '%s\n' 'Before `touch "$HOME/marker"`' '#aida:eval:begin' 'export AIDA_SESSION_ROLE=advisor' '#aida:eval:end' 'After $(touch "$HOME/marker")' > "$fixture/output" ;;
        esac
        env -i HOME="$fixture/home" PATH="$fixture/bin:/usr/bin:/bin" FIXTURE="$fixture" CLI_RC="$cli_rc" SCENARIO="$scenario" \
            "$shell" -f "$fixture/run" > "$fixture/stdout" 2> "$fixture/stderr" || {
                echo "FAIL: $shell $scenario executed marker command"
                exit 1
            }
        tail -n 1 "$fixture/stdout" > "$fixture/state"
        printf 'rc:%s role:%s\n' "$cli_rc" "$expected_role" > "$fixture/expected"
        diff -u "$fixture/expected" "$fixture/state"
        if [ "$cli_rc" -ne 0 ]; then
            cmp "$fixture/output" "$fixture/stderr"
        elif [ "$scenario" != valid ]; then
            sed '$d' "$fixture/stdout" > "$fixture/prose"
            cmp "$fixture/output" "$fixture/prose"
        else
            sed '$d' "$fixture/stdout" > "$fixture/prose"
            printf '%s\n' 'Before `touch "$HOME/marker"`' 'After $(touch "$HOME/marker")' > "$fixture/expected"
            cmp "$fixture/expected" "$fixture/prose"
        fi
        echo "PASS: $shell $scenario"
    done
done

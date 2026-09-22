#!/bin/sh
set -eu

# Regression for TASK-1274: exact-SHA retries stay warm, while a changed source
# file still rebuilds even when the previous normalized timestamp was newer.
repo_root=$(git rev-parse --show-toplevel)
fixture=$(mktemp -d)
trap 'chmod -R u+w "$fixture" 2>/dev/null || true; rm -rf "$fixture"' EXIT

cargo init -q --name mtime_probe "$fixture"
git -C "$fixture" config user.name test
git -C "$fixture" config user.email test@example.invalid
mkdir -p "$fixture/templates/nested"
printf 'fixture\n' > "$fixture/templates/nested/input.txt"
cat > "$fixture/build.rs" <<'EOF'
fn main() {
    println!("cargo:rerun-if-changed=templates/");
}
EOF
git -C "$fixture" add .
git -C "$fixture" commit -qm initial

run_restore() {
    (cd "$fixture" && CARGO_TARGET_DIR="$fixture/target" "$repo_root/ci/restore-git-mtimes")
}
record_checkout() {
    (cd "$fixture" && CARGO_TARGET_DIR="$fixture/target" "$repo_root/ci/restore-git-mtimes" --record)
}
run_build() {
    CARGO_TARGET_DIR="$fixture/target" \
        cargo build --manifest-path "$fixture/Cargo.toml" -v 2>&1
}

run_restore
run_build >/dev/null
record_checkout >/dev/null
git -C "$fixture" checkout -qf HEAD
run_restore
warm=$(run_build)
printf '%s\n' "$warm" | grep -q 'Fresh mtime_probe'

sed -i 's/Hello, world!/Changed/' "$fixture/src/main.rs"
git -C "$fixture" add .
git -C "$fixture" commit -qm changed
run_restore
changed=$(run_build)
printf '%s\n' "$changed" | grep -q 'Compiling mtime_probe'
test "$($fixture/target/debug/mtime_probe)" = Changed
record_checkout >/dev/null

git -C "$fixture" checkout -qf HEAD
run_restore
warm_again=$(run_build)
printf '%s\n' "$warm_again" | grep -q 'Fresh mtime_probe'

echo "restore-git-mtimes: changed sources rebuild; exact-SHA retry is fresh"

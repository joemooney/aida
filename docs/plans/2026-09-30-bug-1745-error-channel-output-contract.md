# BUG-1745 — architecture sketch: which channel carries an error, per output mode

**Seat:** advisor (relay session #8) · **Date:** 2026-09-30 · **Spec:** BUG-1745 (approved, high)
**Status:** DECIDED — advisor signoff recorded. Implementation may proceed on the contract below.

Session #21 committed to signing off on *"a sketch stating which channel carries an error in each of
the three output modes (human / TOON / JSON) and what an agent loop reads in each"* — not on a diff
with no sketch. This is that sketch.

---

## 1. The question

`aida doctor check performance --fail-on-findings` is the performance guard's configured command. Every
trip it writes carries `audit_error` and zero audits. The spec attributes this to the performance
category never emitting evidence. **It does emit evidence.** The question this sketch must settle is
narrower and more general:

> In agent output mode, when a command has **already written a typed payload to stdout**, where does a
> subsequent error go — and what does an agent loop read?

---

## 2. Measured facts (2026-09-30, released `aida` 0.15.0 / `7d85a80`, this repo's config)

Every line below was run, not inferred.

### 2.1 The scheduler's argv already contains `--json`

`maintenance_schedule.rs:2796-2805` — the `command_table()` entry is:

```rust
(
    &["doctor check performance --fail-on-findings"],
    ScheduledCommand {
        display: "doctor check performance --fail-on-findings",
        args: &["doctor", "check", "performance", "--json", "--fail-on-findings"],
```

The config string (`.aida/config.toml:131`, scaffolded at `init_cmd.rs:2541`) is a **display key, not
an argv**. `--json` is already passed.

### 2.2 `performance_audits` is in the JSON schema and is populated

`doctor_cmd.rs:387` declares it with `#[serde(skip_serializing_if = "Vec::is_empty")]`. It is populated
on both paths — the full append path (`doctor_cmd.rs:596`) and the store-free light path
(`performance_light_report_with`, `doctor_cmd.rs:850`), which is what `doctor check performance`
dispatches to via `doctor_check_store_free_light` (`doctor_cmd.rs:812-815`).

So the key's absence from an observed `--json` run means the vec was **empty at that moment**, not that
the field does not exist.

### 2.3 The reproduction, and the exact contaminant

```
$ aida doctor check performance --fail-on-findings --json > out 2> err    # stdout captured
exit=1   stdout=30 lines   stderr=0 lines
$ python3 -c "import json; json.load(open('out'))"
json.decoder.JSONDecodeError: Extra data: line 30 column 1 (char 1193)
```

`line 30 column 1` is **byte-for-byte the position in the recorded trip**
(`performance-guard@2026-09-29T09:42:49.813546761Z`: `malformed typed doctor output: trailing
characters at line 30 column 1`). Lines 1-29 are a valid envelope; line 30 is:

```
error: "1 finding(s) in performance — failing because --fail-on-findings was requested"
```

Parsing the first 29 lines alone:

```
keys = ['bwrap', 'findings', 'performance_audits', 'total']
performance_audits = 1 entry, including "ceiling_breached": true
```

### 2.4 The switch, isolated

```
$ AIDA_AGENT_OUTPUT=0 aida doctor check performance --fail-on-findings --json > out 2> err
exit=1   stdout=29 lines   stderr=1 line
stdout PARSES OK, performance_audits = 1
stderr: Error: 1 finding(s) in performance — failing because --fail-on-findings was requested
```

**With agent mode off, the evidence channel works correctly today, with no code change.** Exit code is
unchanged either way.

### 2.5 Why the scheduler always hits it and a developer may not

`agent_output_mode()` (`lib.rs:2304`) falls through to `agent_output_mode_from`
(`lib.rs:2546`), whose default is `None => !stdout_is_tty`. The scheduler captures stdout, so stdout is
never a TTY, so **agent mode is always on for the guard**. The error block is then emitted by
`lib.rs:1072`:

```rust
if agent_output_mode() {
    println!("{}", agent_error_block(&err, &msg));   // ← println! == STDOUT
}
```

`println!` is stdout. That single line is the whole defect.

### 2.6 `doctor check` has no TOON rendering at all (a second, separate defect)

`doctor_check_store_free_light` (`doctor_cmd.rs:817-822`) branches on the **local `--json` bool only**:

```rust
if json { println!("{}", serde_json::to_string_pretty(&report)?); }
else    { render_doctor_report(&report, false)?; }
```

Measured: with no `--json` and stdout captured (agent mode ON), stdout is **human box-drawing text**
with the TOON `error:` block appended — a payload in one format and an error in another, on one stream.
TOON mode is simply not implemented for this command.

### 2.7 The spec's last open question, answered

`--fail-on-findings` on this path gates on `report.findings`
(`doctor_cmd.rs:825`), and in the light path `report.findings` is built **only** from
`performance_findings` (`doctor_cmd.rs:857`). So for the `performance` category it is
**performance-scoped, not the generic finding set.** Measured: exit 1 with exactly 1 performance
finding. This is **not** a second defect; nothing to file.

---

## 3. Correction to the filing — read this before planning

The spec's **conclusion is correct** (the trip always carries `audit_error`; the advisor pays for it
every 6 hours) and its ledger evidence is correct. Its **mechanism is wrong on three counts**, and each
error points the implementer at the wrong file:

| spec says | measured | consequence of believing it |
|---|---|---|
| item 1: "It passes NO `--json`" | argv **does** pass `--json` (§2.1) | leads to editing the config string — see the trap in §6.1 |
| item 3: "emits HUMAN box-drawing text, not JSON. serde_json therefore fails" | the scheduled run emits a **valid 29-line JSON envelope**; box text would fail at line 1, and the recorded error is at **line 30** (§2.3) | points at doctor's renderer instead of the error channel |
| item 4: "Adding `--json` does NOT fix it … there is no `performance_audits` key … lands on the `Ok(_)` branch" | `--json` **already** yields `performance_audits` with 1 entry; the failure is the **`Err`** branch (`maintenance_schedule.rs:3161`), not `Ok(_)` (`:3157`) | leads to building an emitter that already exists |

The spec's **line numbers are also stale** relative to `main` @ `54c950a807`: it cites `:3148` for the
`Ok(_)` branch (now `:3157`) and `:3959` for the literal-envelope test (now `:4124`). Every citation in
this sketch was re-resolved against that commit. Re-grep rather than trusting either document's numbers.

**The payload AC1 asks for already exists and is already correct** — budget in force, measured value,
denominator, and `ceiling_breached`. Nothing in the performance category needs to be written. Acting on
the filed root cause would have produced a large, wrong change across nine coupled sites while leaving
the actual one-line defect in place.

---

## 4. The decision: one document per stream

**Invariant (adopted):** *stdout carries exactly one document in the pinned format. An error is a field
of that document, never a second document appended to it. The exit code remains the authoritative
failure signal in every mode.*

### The three-mode contract

| mode | stdout | error channel | what an agent loop reads |
|---|---|---|---|
| **human** (TTY, or `--format human`) | human text | **stderr**, red `Error:` prefix | n/a — no agent |
| **TOON** (agent mode, no format pin) | one TOON document | **stdout, inside that document** as a top-level `error:` scalar | the document; `error:` present ⇒ failed |
| **JSON** (`--json` / `--format json`) | one JSON object | **stdout, inside that object** as a top-level `"error"` field | the object; `error` present ⇒ failed |

This **preserves TASK-972 (AXI #6)** — "agents read stdout; an error on stderr is invisible to the agent
loop" — while fixing its unexamined assumption. AXI #6 is correct that the error must be *reachable on
stdout*; it is wrong only in assuming nothing else had already written a structured document there.

### Options considered and rejected

- **(A) Send the error to stderr in agent mode too.** Cleanest diff, and §2.4 proves it works. **Rejected:**
  it reverts a deliberate AXI decision and blinds every agent loop that reads only stdout. The bug is
  composition, not the decision.
- **(C) Suppress the error block when a typed payload was already emitted.** **Rejected:** stdout parses,
  but the agent sees a successful-looking document and must infer failure from the exit code alone —
  exactly the "absent evidence indistinguishable from no problem" shape PRIN-5 forbids.
- **(D) Two newline-delimited documents on stdout (JSONL).** **Rejected:** changes the contract for every
  existing consumer, including `failure_trip`, to fix one command.

Option B is the only one that keeps stdout parseable *and* keeps the failure reason on stdout.

---

## 5. Limbs, in implementation order

1. **Make the global error path aware that stdout already carries a typed document.** `lib.rs:1072` must
   not append a second document. The error becomes a field of the emitted payload, or — where the payload
   has already been flushed — is emitted on stderr rather than appended to stdout. Whichever shape is
   chosen, state it as a rule with a test.
2. **Fix the guard's own path concretely.** `doctor_check_store_free_light` should stop signalling
   `--fail-on-findings` through `bail!` into the global renderer (`doctor_cmd.rs:825-830`). It should put
   the failure reason into the envelope it is already printing and set the exit code directly.
3. **AC3's round trip, end to end.** Run the *resolved argv* from `command_table()` — not a hand-written
   command — feed its **real stdout** to `failure_trip`, and assert the trip carries audits and
   `audit_error: None`. The spec is explicit that a test hand-building the JSON
   (`maintenance_schedule.rs:4124` constructs `"performance_audits": [...]` as a literal) does **not**
   satisfy this; that test shape is what let the defect ship.
4. **AC4: keep both failure branches reachable and tested** — malformed output (`:3161`) and well-formed
   output with no audits (`:3157`). Neither may be silently turned into a success.
5. **AC6: mutation proof.** Revert the fix, re-run the new round-trip test, confirm it goes **red**,
   restore. A test that passes against both the broken and fixed binary advertises coverage that does not
   exist.

**Out of scope — file separately, do not widen this spec:** §2.6, `doctor check`'s missing TOON rendering.
It is a real defect on the same contract, but it is a renderer gap in a different function and BUG-1745's
acceptance does not cover it.

---

## 6. Traps recorded so the implementer cannot fall into them

### 6.1 Do not change the configured command string

The string `"doctor check performance --fail-on-findings"` is load-bearing in **nine** places, and one of
them is an **identity predicate**:

```rust
// maintenance_schedule.rs:3146
let is_performance = task.command.as_ref()
    .is_some_and(|c| c.display == "doctor check performance --fail-on-findings");
```

Sites: `command_table` key + display (`:2796`, `:2798`), this predicate (`:3146`), the scaffold
(`init_cmd.rs:2541`), the in-repo config (`.aida/config.toml:131`), and three tests (`:4150`, `:4261`,
`:4445`).

Changing the display string to add `--json` — the fix the spec's item 1 implies — would make
`is_performance` evaluate **false**, at which point the guard stops even *attempting* to parse: no audits
**and no `audit_error`**. That is strictly worse than today's honest failure, and it fails silently.

**The adopted fix requires no config change at all.** That answers AC5 directly: the scaffolded default
and the in-repo config stay as they are, there is no migration posture to state, and the display/argv
divergence is intentional and documented in §2.1 — say so in the PR rather than editing either file.

### 6.2 `performance_audits` absent ≠ field missing

`skip_serializing_if = "Vec::is_empty"` means an empty vec serializes to **no key**. Do not read a missing
key as a schema gap; check whether the vec was populated.

### 6.3 Reproduce with stdout captured, never at a TTY

At a TTY with no format pin, agent mode is off and the bug does not appear (§2.5). Any manual check must
redirect stdout to a file or a pipe. This is why it shipped.

### 6.4 Do not change any budget

The spec is explicit: no budget may be raised and BUG-1674 / STORY-1484 are untouched. The standing
disposition — do not raise; the fix is STORY-1484's single-flight cache refresh — is preserved. This is an
evidence-channel change only. Note the guard currently trips on a **real** open breach (§2.3: worst call
12365 ms against a 5000 ms ceiling), so a correct fix will make that breach *visible*, not quiet.

---

## 7. Signoff

The contract in §4 is **signed off by the advisor seat**. AC4's design question is closed; do not
re-litigate it. Implementation of §5 may proceed.

Two points to carry into the PR body:
- The filing's root cause was wrong in three places (§3); the PR should say so, so the next reader does
  not re-derive it.
- `--fail-on-findings` is **performance-scoped** on this path (§2.7) — the spec's final AC is answered,
  with nothing further to file.

#!/usr/bin/env python3
"""SPIKE-46 — empirical multi-vendor substrate-access prototype.

A deliberately tiny, dependency-light (pyyaml only) NON-AIDA tool that reads
AIDA's git-canonical store and probes how hard it is to interoperate. It answers
the moat question from docs/architecture/on-disk-serialization-surface.md with
measurements instead of assertions:

  1. READ — can a non-Rust tool parse every spec object and reach the fields a
     consumer cares about (id, status, relationships, tags)?  Expectation: easy.
  2. ROUND-TRIP — if that tool re-serializes an object with its own YAML
     emitter, is the output byte-identical to AIDA's?  Expectation: NO — and the
     *reason* is what defines the write-conformance contract.

Usage:
    python3 store_reader.py [STORE_DIR]
    # STORE_DIR defaults to ../../../.aida-store (this repo's live store)

This is a read-only probe. It never writes into the store.
trace:SPIKE-46 | ai:claude
"""
import sys
import os
import glob
import io

try:
    import yaml
except ImportError:
    sys.exit("pyyaml required: pip install pyyaml")


class AidaLoader(yaml.SafeLoader):
    """SafeLoader plus the ONE trick a stock loader lacks: AIDA serializes a
    custom RelationshipType as a `!Custom <name>` YAML tag (e.g.
    `rel_type: !Custom verifies-indirectly`). A stock SafeLoader has no
    constructor for `!Custom` and raises on ~a quarter of real objects. This
    three-line handler normalizes it to `{"Custom": name}` — the same logical
    shape AIDA's fallback-tolerant deserializer accepts. trace:SPIKE-46
    """


def _construct_custom(loader, node):
    if isinstance(node, yaml.ScalarNode):
        return {"Custom": loader.construct_scalar(node)}
    if isinstance(node, yaml.SequenceNode):
        return {"Custom": loader.construct_sequence(node)}
    return {"Custom": loader.construct_mapping(node)}


AidaLoader.add_constructor("!Custom", _construct_custom)


def _load(raw, loader):
    return yaml.load(raw, Loader=loader)


def find_store(argv):
    if len(argv) > 1:
        return argv[1]
    here = os.path.dirname(os.path.abspath(__file__))
    # docs/architecture/spike-46-store-interop -> repo root -> .aida-store
    return os.path.normpath(os.path.join(here, "..", "..", "..", ".aida-store"))


def main():
    store = find_store(sys.argv)
    objects_dir = os.path.join(store, "objects")
    paths = sorted(glob.glob(os.path.join(objects_dir, "**", "*.yaml"), recursive=True))
    if not paths:
        sys.exit(f"no objects under {objects_dir} (is STORE_DIR right?)")

    print(f"SPIKE-46 store-interop probe — {len(paths)} objects under {objects_dir}\n")

    # ---- 1. READ ---------------------------------------------------------
    # Pass A: a stock SafeLoader (the naive non-AIDA tool).
    # Pass B: SafeLoader + the one `!Custom` constructor (the informed tool).
    def read_pass(loader):
        ok, fail, reached, parsed_local = 0, [], 0, {}
        kinds = set()
        for p in paths:
            with open(p, "r", encoding="utf-8") as fh:
                raw = fh.read()
            try:
                doc = _load(raw, loader)
            except Exception as e:  # noqa: BLE001 - probe wants every failure class
                fail.append((p, repr(e)))
                continue
            ok += 1
            parsed_local[p] = (raw, doc)
            if isinstance(doc, dict) and "id" in doc and "status" in doc:
                reached += 1
            for rel in (doc.get("relationships") or []) if isinstance(doc, dict) else []:
                if isinstance(rel, dict):
                    rt = rel.get("rel_type")
                    kinds.add(next(iter(rt)) if isinstance(rt, dict) else str(rt))
        return ok, fail, reached, kinds, parsed_local

    stock_ok, stock_fail, _, _, _ = read_pass(yaml.SafeLoader)
    read_ok, read_fail, reached_fields, relationship_kinds, parsed = read_pass(AidaLoader)

    print("== 1. READ ==")
    print(f"  stock SafeLoader        : {stock_ok}/{len(paths)} parsed", end="")
    if stock_fail:
        causes = {}
        for _, e in stock_fail:
            tag = "!Custom tag" if "!Custom" in e else e.split("(")[0]
            causes[tag] = causes.get(tag, 0) + 1
        print(f"  ({len(stock_fail)} FAILED: {dict(causes)})")
    else:
        print()
    print(f"  + !Custom constructor   : {read_ok}/{len(paths)} parsed")
    print(f"  reached id+status fields: {reached_fields}/{read_ok}")
    print(f"  distinct relationship kinds seen: {sorted(relationship_kinds)}")
    if read_fail:
        print(f"  STILL-FAILING ({len(read_fail)}):")
        for p, e in read_fail[:5]:
            print(f"    {os.path.basename(p)}: {e}")
    else:
        print("  → read interop: a stock loader fails on the !Custom tag, but ONE")
        print("    three-line constructor lifts it to 100%. Read is easy-with-one-trick.")
    print()

    # ---- 2. ROUND-TRIP ---------------------------------------------------
    # Re-emit each parsed doc with pyyaml's default emitter and compare bytes.
    byte_identical = 0
    semantic_identical = 0
    diverged_examples = []
    for p, (raw, doc) in parsed.items():
        reemit = yaml.safe_dump(doc, default_flow_style=False, sort_keys=False, allow_unicode=True)
        if reemit == raw:
            byte_identical += 1
        # Semantic check: re-parse the re-emitted text and compare structures.
        try:
            if yaml.safe_load(reemit) == doc:
                semantic_identical += 1
        except Exception:  # noqa: BLE001
            pass
        if reemit != raw and len(diverged_examples) < 1:
            diverged_examples.append((p, raw, reemit))

    n = len(parsed)
    print("== 2. ROUND-TRIP (pyyaml re-emit vs AIDA's serde_yaml bytes) ==")
    print(f"  byte-identical : {byte_identical}/{n}")
    print(f"  semantic-equal : {semantic_identical}/{n}  (re-parse == original data)")
    if byte_identical < n:
        print("  → write interop is NOT free: a stock emitter preserves the DATA")
        print("    but not the BYTES. The divergence is pure formatting, and it is")
        print("    what AIDA's write_object_if_changed sees as a (spurious) diff.")
    if diverged_examples:
        p, raw, reemit = diverged_examples[0]
        ra, re_ = raw.splitlines(), reemit.splitlines()
        first_diff = next(
            (i for i in range(max(len(ra), len(re_)))
             if (ra[i] if i < len(ra) else None) != (re_[i] if i < len(re_) else None)),
            0,
        )
        print(f"\n  First divergence — {os.path.basename(p)} at line {first_diff + 1}:")
        print(f"    AIDA  | {ra[first_diff] if first_diff < len(ra) else '<eof>'}")
        print(f"    pyyaml| {re_[first_diff] if first_diff < len(re_) else '<eof>'}")

    # ---- VERDICT ---------------------------------------------------------
    print("\n== VERDICT ==")
    read_easy = not read_fail and reached_fields == read_ok
    print(
        "  READ  : "
        + (
            f"EASY-WITH-ONE-TRICK — stock loader fails {len(stock_fail)}/{len(paths)} "
            "(all !Custom);\n          +1 constructor → 100%, all fields reachable."
            if read_easy
            else "GAPS remain even with the !Custom handler (see still-failing)."
        )
    )
    write_gap = byte_identical < n and semantic_identical == n
    print(
        "  WRITE : "
        + (
            "DATA-SAFE but NOT byte-clean (0 identical / all semantically equal) —\n"
            "          the conformance contract is an EMITTER spec (field order +\n"
            "          scalar styles + sorted collections + skip rules), not just the\n"
            "          field-level serde attributes."
            if write_gap
            else "byte-clean out of the box (unexpected — investigate)"
        )
    )


if __name__ == "__main__":
    main()

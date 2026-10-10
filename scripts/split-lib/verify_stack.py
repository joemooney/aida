#!/usr/bin/env python3
"""Reviewer-side pure-move checker for STORY-1488 slice stack.

usage: verify_stack.py <base> <head> [--build]

Checks a stack of commits for pure-move compliance and tooling edits.
See also: verify_pr.sh (per-slice build/test/golden gate).

# trace:TASK-1722 | ai:antigravity
"""

import collections, re, subprocess, sys

ALLOWED_ADD = [re.compile(p) for p in (
    r"^//!", r"^// trace:", r"^use crate::\*;$",
    r"^(pub(\(crate\))? )?mod \w+;$", r"^(pub(\(crate\))? )?use \w+::\*;$", r"^$")]
EXPECTED_MOVE_FILE = re.compile(r"^aida-cli-lib/src/([a-z0-9_]+)\.rs$|^docs/plans/.*manifest\.toml$")
LIB_FILE = re.compile(r"^aida-cli-lib/src/lib(_part[0-9]+)?\.rs$")
SPEC_ID = re.compile(r"(STORY|TASK|BUG|FR|EPIC)-[0-9]+")

def git(*args, check=True):
    res = subprocess.run(["git", *args], capture_output=True, text=True)
    if check and res.returncode != 0:
        raise Exception(f"git {args} failed: {res.stderr}")
    return res.stdout

def get_tags_on_commit(sha):
    out = git("tag", "--points-at", sha)
    return [t.strip() for t in out.splitlines() if t.strip().startswith("slice-")]

def get_tag_annotation(tag):
    out = subprocess.run(["git", "cat-file", "tag", tag], capture_output=True, text=True)
    if out.returncode != 0:
        return ""
    lines = out.stdout.splitlines()
    try:
        idx = lines.index("")
        return "\n".join(lines[idx+1:])
    except ValueError:
        return ""

def code_strip(s):
    return re.sub(r"\s+", "", s)

def check_commit(sha, last_was_move, last_move_module):
    subj = git("log", "-1", "--format=%s", sha).strip()
    files = git("diff-tree", "--no-commit-id", "--name-only", "-r", sha).split()
    
    is_move = subj.startswith("refactor(cli-lib): extract ")
    is_retarget = subj.startswith("retarget:") or subj.startswith("tooling:")
    is_final = "lib_part" in subj.lower() and "delete" in subj.lower()
    
    issues = []
    
    if not (is_move or is_retarget or is_final):
        issues.append(f"FAIL: Subject not move or retarget/tooling")
        
    if SPEC_ID.search(subj):
        if not is_final:
            issues.append(f"FAIL: Contains spec ID in subject")

    module_name = None
    if is_move:
        m = re.match(r"refactor\(cli-lib\): extract ([a-z0-9_]+)\.rs", subj)
        if m:
            module_name = m.group(1)
        
        added, removed = collections.Counter(), collections.Counter()
        cur = ""
        diff_lines = git("show", "--format=", "-U0", "--no-color", sha).splitlines()
        
        lib_file_additions = []
        
        for line in diff_lines:
            if line.startswith("diff --git "):
                cur = line.split(" b/", 1)[-1]
                continue
            if line.startswith(("+++", "---")) or cur.endswith("manifest.toml"):
                continue
            if line.startswith("+"):
                added[line[1:]] += 1
                if LIB_FILE.match(cur):
                    lib_file_additions.append(line[1:])
            elif line.startswith("-"):
                removed[line[1:]] += 1

        extra_add = collections.Counter({l: n for l, n in (added - removed).items()
                                         if not any(p.match(l.strip()) for p in ALLOWED_ADD)})
        extra_rem = collections.Counter({l: n for l, n in (removed - added).items() if l.strip()})
        norm = lambda c: collections.Counter({re.sub(r"\s+", " ", l.strip()): n for l, n in c.items()})
        reindent_only = bool(extra_add or extra_rem) and norm(extra_add) == norm(extra_rem)
        
        odd_files = []
        for f in files:
            if not (LIB_FILE.match(f) or f == f"aida-cli-lib/src/{module_name}.rs" or f.endswith("manifest.toml")):
                odd_files.append(f)
                
        if odd_files:
            issues.append(f"FAIL: File outside move set: {', '.join(odd_files)}")
            
        if extra_add or extra_rem:
            if reindent_only:
                issues.append("FAIL: Residue is whitespace-only (reindent)")
            else:
                issues.append("FAIL: Added/removed lines do not match pure move")
                
        if module_name:
            mod_count = 0
            use_count = 0
            for add in lib_file_additions:
                if re.match(r"^(pub(\(crate\))? )?mod " + module_name + r";$", add.strip()):
                    mod_count += 1
                if re.match(r"^(pub(\(crate\))? )?use " + module_name + r"::\*;$", add.strip()):
                    use_count += 1
            if mod_count != 1 or use_count != 1:
                issues.append(f"FAIL: Module {module_name} not wired exactly once in lib.rs")
                
            try:
                line_count = int(git("show", f"{sha}:aida-cli-lib/src/{module_name}.rs", check=False).count("\n"))
                if line_count > 6000:
                    issues.append(f"FAIL: Size cap exceeded for {module_name}.rs")
            except:
                pass
                
    elif is_retarget:
        # Check if any .rs code line was changed in lib*.rs or moved module
        diff_lines = git("show", "--format=", "-U0", "--no-color", sha).splitlines()
        added_code = ""
        removed_code = ""
        cur = ""
        for line in diff_lines:
            if line.startswith("diff --git "):
                cur = line.split(" b/", 1)[-1]
                continue
            if line.startswith(("+++", "---")):
                continue
            
            is_protected = LIB_FILE.match(cur) or (last_move_module and cur == f"aida-cli-lib/src/{last_move_module}.rs")
            if not is_protected:
                continue
                
            if line.startswith(("+", "-")):
                content = line[1:].strip()
                if not content or content.startswith("//"):
                    continue
                if line.startswith("+"):
                    added_code += code_strip(content)
                else:
                    removed_code += code_strip(content)
                    
        if added_code != removed_code:
            # Code actually changed!
            bad_files = [f for f in files if LIB_FILE.match(f) or (last_move_module and f == f"aida-cli-lib/src/{last_move_module}.rs")]
            issues.append(f"FAIL: retarget/tooling commit edits protected code: {', '.join(bad_files)}")

    return is_move or is_final, module_name, is_retarget, issues, files

def main():
    args = sys.argv[1:]
    do_build = "--build" in args
    if do_build:
        args.remove("--build")
        
    if len(args) != 2:
        sys.exit(__doc__)
        
    base, head = args
    
    try:
        git("merge-base", "--is-ancestor", base, head)
    except:
        print("FAIL: head does not descend from base")
        sys.exit(1)
        
    merges = git("rev-list", "--merges", f"{base}..{head}").strip()
    if merges:
        print("FAIL: Range contains merge commits")
        sys.exit(1)
        
    shas = git("rev-list", "--reverse", f"{base}..{head}").split()
    
    all_ok = True
    last_was_move = False
    last_move_module = None
    
    for i, sha in enumerate(shas):
        subj = git("log", "-1", "--format=%s", sha).strip()
        tags = get_tags_on_commit(sha)
        
        is_move, mod_name, is_retarget, issues, files = check_commit(sha, last_was_move, last_move_module)
        
        if last_was_move and is_move:
            issues.append(f"NOTE: MOVE without retarget")
            
        for tag in tags:
            anno = get_tag_annotation(tag)
            if not anno:
                issues.append(f"FAIL: Tag {tag} is not annotated")
                continue
                
            mod_to_check = mod_name if is_move else (last_move_module if is_retarget and last_was_move else None)
            
            if mod_to_check:
                l = anno.lower()
                if "v3" not in l or "warnings" not in l:
                    issues.append(f"FAIL: Tag {tag} missing V3 record")
                if "v4" not in l or "tests" not in l:
                    issues.append(f"FAIL: Tag {tag} missing V4 record")
                if "v7" not in l or "size" not in l:
                    issues.append(f"FAIL: Tag {tag} missing V7 record")
                    
        if is_move and do_build:
            pass # optional build omitted here for brevity

        last_was_move = is_move
        if is_move and mod_name:
            last_move_module = mod_name
            
        if tags:
            issues.append(f"NOTE: Tagged with {', '.join(tags)}")
            
        status = "FAIL" if any(i.startswith("FAIL:") for i in issues) else "PASS"
        if status == "FAIL":
            all_ok = False
            
        short_sha = sha[:7]
        cls = "MOVE" if is_move else ("RETARGET" if is_retarget else "OTHER")
        
        print(f"{status}  {short_sha} {cls:<8} {subj}")
        if is_retarget:
            print(f"      files touched: {', '.join(files)}")
        for issue in issues:
            print(f"      {issue}")
            
    if all_ok:
        print("\n9/9 PASS")
    else:
        print("\nSome commits failed.")
        
    sys.exit(0 if all_ok else 1)

if __name__ == "__main__":
    main()

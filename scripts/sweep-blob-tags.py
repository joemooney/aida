#!/usr/bin/env python3
# trace:TASK-1566 | ai:claude | impl:2026-10-03 | by:joe
import os
import sys
import yaml
import subprocess
from pathlib import Path

def find_store_dir():
    # Sibling worktrees don't have .aida-store automatically, they resolve it.
    # We can ask `aida env AIDA_STORE_DIR` or similar. Wait, does aida env exist?
    # Let's just find the store by running aida and looking at where it writes, or just assume the canonical store is at .aida-store in the main worktree or current dir.
    # Actually, aida doesn't have an `env` command according to the help text.
    # But wait, in a sibling worktree, the store is either a symlink or handled by Rust.
    # If this script is run from the worktree, it can just use `git rev-parse --git-common-dir` or similar?
    try:
        git_common_dir = subprocess.check_output(['git', 'rev-parse', '--git-common-dir']).decode('utf-8').strip()
        store_path = Path(git_common_dir).parent / '.aida-store'
        if store_path.exists():
            return store_path
    except Exception:
        pass
    if Path('.aida-store').exists():
        return Path('.aida-store')
    return None

def main():
    apply = '--apply' in sys.argv
    store_dir = find_store_dir()
    if not store_dir:
        print("Error: Could not find .aida-store directory.")
        sys.exit(1)
        
    objects_dir = store_dir / 'objects'
    if not objects_dir.exists():
        print(f"Error: {objects_dir} does not exist.")
        sys.exit(1)

    candidates = 0
    repaired = 0
    
    for yaml_file in objects_dir.rglob('*.yaml'):
        with open(yaml_file, 'r', encoding='utf-8') as f:
            content = f.read()
            
        try:
            # We must load with a YAML parser to check the list items cleanly
            obj = yaml.safe_load(content)
        except Exception as e:
            print(f"Skipping {yaml_file} due to parsing error: {e}")
            continue
            
        if not obj or not isinstance(obj, dict):
            continue
            
        tags = obj.get('tags')
        if not tags or not isinstance(tags, list):
            continue
            
        has_blob = False
        new_tags_set = set()
        
        for tag in tags:
            tag_str = str(tag)
            # A whitespace blob detector: tag contains whitespace
            import re
            if re.search(r'\s+', tag_str):
                has_blob = True
                # The repair logic: split on whitespace
                for split_tag in tag_str.split():
                    new_tags_set.add(split_tag)
                print(f"[{obj.get('id')}] BLOB DETECTED: {repr(tag_str)}")
                print(f"[{obj.get('id')}] PROPOSED SPLIT: {repr(tag_str.split())}")
            else:
                new_tags_set.add(tag_str)
                
        if has_blob:
            candidates += 1
            if apply:
                spec_id = obj.get('spec_id') or obj.get('id')
                new_tags_str = ",".join(sorted(new_tags_set))
                print(f"[{spec_id}] Applying repair: aida edit {spec_id} --tags \"{new_tags_str}\"")
                res = subprocess.run(['aida', 'edit', spec_id, '--tags', new_tags_str], capture_output=True, text=True)
                if res.returncode == 0:
                    repaired += 1
                else:
                    print(f"[{spec_id}] Error applying repair: {res.stderr}")

    print(f"\nFound {candidates} objects with blob tags.")
    if apply:
        print(f"Successfully repaired {repaired} objects.")
    else:
        print("Run with --apply to perform the repair.")

if __name__ == '__main__':
    main()

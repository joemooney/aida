"""SPIKE-86 layer classifier: maps a repo path to an architectural layer and
measures per-layer churn. Run with python3 (never as a shell script).
trace:SPIKE-86 | ai:claude
Rules are evaluated in order; first match wins."""
import re, sys, subprocess, collections, os, argparse
RULES = [
 ("dispatcher", r'^(aida-cli/src/main\.rs|aida-cli-lib/src/lib\.rs)$'),
 ("docs",       r'^PROMPT_HISTORY\.md$'),
 ("control",    r'aida-cli-lib/src/(doctor_cmd|status_cleanup)'),
 ("ci_build",   r'aida-cli-lib/src/dev_cmd|^\.gitlab-ci|^aida-core/build\.rs'),
 ("docs",       r'^(docs/|README|OVERVIEW|CHANGELOG|CLAUDE\.md|AGENTS\.md|WHY-AIDA|REVIEW\.md)'),
 ("scaffold",   r'^(aida-core/templates/|templates/|\.claude/|\.aida/discipline|plugins/|aida-core/src/scaffolding/|aida-core/src/templates\.rs|aida-cli-lib/src/(init_|scaffold|rules_|first_run|upgrade_cmd|manual))'),
 ("ci_build",   r'^(\.github/|ci/|scripts/|Makefile|Cargo\.(toml|lock)|docker|Dockerfile|bench/|pnpm|helper/|[^/]+/Cargo\.toml)'),
 ("tests_dir",  r'^tests/|/tests/|_tests?\.rs$'),
 ("surface",    r'^(aida-tui/|aida-web-react/|aida-server/|aida-generate-types/|proto/|shared/)|aida-cli-lib/src/(mcp|statusline|statusbar|status_display|status_cmd|glyph|help_|cli\.rs|toon|terminal_cmd|pane_host|not_found|completion|awaiting_you|wiki|deep_link|edit_buffer|prompts|workflow_hints|alias|user_alias|do_dispatch|generated/|client|server_cmd|tail_cmd|focus)'),
 ("intent",     r'aida-cli-lib/src/(reconstitute|criteria|trace_cmd|intent|contradictions|interview|dryrun|exposition|harvest|evaluator|research|docs\.rs|doc_cmd|digest|changelog|brief_cmd|memories|triage_cmd|intake|import_plan|plan_cmd|goal|decide_cmd|human_audit|capture)|aida-core/src/(ears_lint|docs_review|ai/|provenance|analytics|report|rollup)'),
 ("control",    r'aida-cli-lib/src/(queue|drain|orchestra|auto|advisor|review|reviewer|graded_review|merge_|integrate|pr_|ship|gate|ci_|freshness_gate|protocol|locking_gate|worktree_scope_gate|criteria_gate|supervis|schedule|maintenance|seat|runaway|stranded|requeue|punt|human|mailbox|presence|coordination|dispatch|calibration|complexity_cal|effort_cal|events|event_wait|exit_signal|watch|monitor|notify|triage_lease|findings|zen|solo|lock|last_drain|state_snapshot|pending_approval|stacks|rebase|defer|assign|focus|backlog|burndown|metrics|health|usage|token_ledger|field_study|compete|drive_|autopilot|global_queue|orchestration|implementer_preflight|machine_readiness|dispatch_health|identity_guard|rule_violation|permissions|trusted_config|role_cmd|team)|aida-core/src/(gates|pickability|dispenser|liveness|idle|daemon|lock|mailbox|dispatch_routing|review_config|lifecycle|conflict|telemetry|team|deps_sweep|rebase)'),
 ("execution",  r'aida-cli-lib/src/(worktree|session|agent_|claude_agents|worker|process_|headless|sandbox|forge|vendor_activity|remote_|network_retry|shell_eval|test_env|pr_claim|gitlab|stacks)|aida-core/src/(worktree|agents_config|external_tool_output|workspace|idle)'),
 ("store",      r'aida-core/src/|aida-cli-lib/src/(store|cache|db_cmd|graph|relationship|rel_def|related_edge|type_cmd|feature|node|load_cmd|lint_cmd|import_export|mass_change|archive|comment|history|record_cmd|schema|git_backend|config|project_cap|lifecycle_cmd|deps|tracker|external_import|report_cmd|export)'),
]
C = [(n, re.compile(p)) for n,p in RULES]
def layer(path):
    # pre-STORY-772 the CLI modules lived in aida-cli/src/; treat them as their aida-cli-lib successors
    if path.startswith("aida-cli/src/") and path!="aida-cli/src/main.rs":
        path="aida-cli-lib/src/"+path[len("aida-cli/src/"):]
    for n,rx in C:
        if rx.search(path): return n
    return "other"

def fixpath(p):
    """Resolve numstat rename notation ('a/{x => y}/z', 'x => y') to the new path."""
    if "=>" in p: p=re.sub(r'\{[^}]*=> ([^}]*)\}',r'\1',p).split(" => ")[-1]
    return p

def numstat_commits(rev, since, cap):
    """Yield (sha, rows) for non-merge commits reachable from rev since the anchor,
    skipping commits whose total changed lines exceed cap (mechanical moves)."""
    out=subprocess.run(["git","log",rev,"--since="+since,"--no-merges","--numstat","--format=@@%H"],capture_output=True,text=True,check=True).stdout
    kept=[]; skipped=[]
    for b in [b for b in out.split("@@") if b.strip()]:
        lines=b.splitlines(); sha=lines[0].strip()
        rows=[(a,d,fixpath(p)) for a,d,p in (r.split("	") for r in lines[1:] if len(r.split("	"))==3)]
        tot=sum(int(a)+int(d) for a,d,p in rows if a!="-")
        (skipped if tot>cap else kept).append((sha,rows))
    return kept, skipped

def churn(rev, since, cap):
    kept,skipped=numstat_commits(rev,since,cap)
    commits=collections.Counter(); lines=collections.Counter(); touches=collections.Counter()
    for sha,rows in kept:
        seen=set()
        for a,d,p in rows:
            l=layer(p); seen.add(l); touches[l]+=1
            if a!="-": lines[l]+=int(a)+int(d)
        for l in seen: commits[l]+=1
    return dict(kept=len(kept), skipped=len(skipped), commits=commits, lines=lines, touches=touches)

if __name__ == "__main__":
    ap=argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--rev",default="9154924fd2"); ap.add_argument("--since",required=True)
    ap.add_argument("--cap",type=int,default=5000)
    a=ap.parse_args(); r=churn(a.rev,a.since,a.cap)
    print(f"rev={a.rev} since={a.since} cap={a.cap} commits_counted={r['kept']} excluded_over_cap={r['skipped']}")
    tl=sum(r['lines'].values())
    for l,_ in r['touches'].most_common():
        print(f"  {l:12} commits={r['commits'][l]:5} file-touches={r['touches'][l]:5} lines={r['lines'][l]:7} {100*r['lines'][l]/tl:5.1f}%")

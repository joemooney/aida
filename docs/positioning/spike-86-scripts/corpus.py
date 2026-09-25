"""SPIKE-86 spec-corpus analysis: classifies every work spec in the canonical
AIDA store by layer and thesis, splits machine-filed from authored specs, and
measures acceptance-criteria coverage with a reimplementation of
parse_acceptance_criteria (aida-cli-lib/src/criteria.rs).

Reads the store at a pinned git revision (default: aida-store 788c4e050d), so a
third party reproduces the numbers with:

    git fetch origin aida-store
    python3 docs/positioning/spike-86-scripts/corpus.py

Windows are created_at within N days of NOW (2026-09-24T00:00Z).
Requires PyYAML. Run with python3, never as a shell script.
trace:SPIKE-86 | ai:claude
"""
import yaml,re,collections,datetime,sys,subprocess,argparse
ap=argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument("--store-rev",default="788c4e050d")
ap.add_argument("--list30",action="store_true",help="print the 30-day per-spec classification")
ARGS=ap.parse_args()
NOW=datetime.datetime(2026,9,24,tzinfo=datetime.timezone.utc)
def load(rev):
    names=[n for n in subprocess.run(["git","ls-tree","-r","--name-only",rev,"objects"],capture_output=True,text=True,check=True).stdout.split() if n.endswith(".yaml")]
    blob=subprocess.run(["git","cat-file","--batch"],input="".join(f"{rev}:{n}\n" for n in names).encode(),capture_output=True,check=True).stdout
    out=[]; i=0
    while i<len(blob):
        nl=blob.index(b"\n",i); size=int(blob[i:nl].split()[2]); body=blob[nl+1:nl+1+size]; i=nl+1+size+1
        try: d=yaml.safe_load(body.decode("utf-8"))
        except Exception: continue
        if isinstance(d,dict) and "spec_id" in d: out.append(d)
    return out
specs=load(ARGS.store_rev)
def dt(s):
    return datetime.datetime.fromisoformat(str(s).replace('Z','+00:00')[:26]+'+00:00') if s else None
# --- replicate aida-cli-lib/src/criteria.rs parse_acceptance_criteria (headed + inline) ---
def bullet(t):
    m=re.match(r'^(AC|A)(\d+)[.:]',t,re.I)
    if m: return t
    for p in ("- [ ] ","- [x] ","- [X] ","- ","* "):
        if t.startswith(p): r=t[len(p):].strip(); return r or None
    m=re.match(r'^\d+[.)]\s+(.*)',t)
    if m: return m.group(1).strip() or None
    return None
def collect(lines):
    out=[];blank=False
    for line in lines:
        t=line.strip()
        if t.startswith('```') or t.startswith('#'): break
        if not t: blank=True; continue
        b=bullet(t)
        if b: out.append(b); blank=False; continue
        if blank: break
    return out
def criteria(desc):
    desc=desc or ''; L=desc.splitlines(); head=[]; ins=False
    for line in L:
        t=line.strip()
        if t.startswith('#'):
            title=t.lstrip('#').strip()
            if ins and title: break
            if title.lower()=='acceptance' or title.lower().startswith('acceptance criteria'): ins=True; continue
        if ins: head.append(line)
    h=collect(head); inl=[]
    for i,line in enumerate(L):
        m=line.strip().strip('*_').strip().lower()
        if m in ('acceptance:','acceptance criteria:'): inl=collect(L[i+1:]); break
    return h+inl
# --- layer classifier (tags first, then title keywords) ---
LAYER_RX=[
 ('surface',  r'\b(tui|web|dashboard|react|statusline|statusbar|mcp|glyph|render|palette|wiki|help|cli-ux|ux|toon|json output|--json|output format|display|keybinding)\b'),
 ('control',  r'\b(queue|drain|orchestrat\w*|lease|verdict|review\w*|merge|integrat\w*|gate|auto-?complete|auto-bump|advisor|seat|phase|scheduler?|mailbox|presence|escalat\w*|autopilot|supervis\w*|ship|pr\b|batch|punt|requeue|rework|findings|shelv\w*|zen|drive|autonomy|ci|dispatch|fleet|decide|human|triage)\b'),
 ('execution',r'\b(worktree|session|agent adapter|headless|forge|vendor|codex|gemini|agy|sandbox|spawn|launch\w*|harness|claude code|process)\b'),
 ('intent',   r'\b(criteria|acceptance|trace|traceability|reconstitut\w*|intent|contradiction\w*|harvest|why|clarif\w*|interview|exposition|ears|north-?star|capture|provenance|compiler)\b'),
 ('store',    r'\b(store|cache|graph|relationship\w*|edge|id|ids|yaml|orphan|aida-store|lifecycle|history|search|import|export|archive|schema|git-canonical|substrate|sqlite|comment\w*|tag\w*)\b'),
]
TAGMAP={'tui':'surface','mcp':'surface','web':'surface','ux':'surface','orchestrator':'control','autonomy':'control','aida:queue':'control','drain':'control','review':'control','verdict':'control','merge':'control','worktree':'execution','session':'execution','reconstitution':'intent','acceptance-criteria':'intent','north-star':'intent','northstar':'intent','traceability':'intent','storage':'store','cache':'store','graph':'store'}
def layer(s):
    tags=[str(t).lower() for t in (s.get('tags') or [])]
    votes=collections.Counter()
    for t in tags:
        base=t.split(':')[0] if not t.startswith('aida:') else ':'.join(t.split(':')[:2])
        if base in TAGMAP: votes[TAGMAP[base]]+=2
        if t in TAGMAP: votes[TAGMAP[t]]+=2
    title=(s.get('title') or '').lower()
    for n,rx in LAYER_RX:
        if re.search(rx,title): votes[n]+=1
    if not votes: return 'unclassified'
    order=['control','execution','surface','intent','store']
    return max(votes, key=lambda k:(votes[k], -order.index(k)))
TYPES_WORK={'Story','Bug','Task','Epic','Spike','ChangeRequest','Feature','Functional','NonFunctional','Requirement'}
def window(days):
    return [s for s in specs if dt(s.get('created_at')) and (NOW-dt(s.get('created_at'))).days<days]
def report(label,ss):
    ss=[s for s in ss if str(s.get('req_type')) in TYPES_WORK]
    c=collections.Counter(layer(s) for s in ss)
    n=len(ss)
    print(f"\n[{label}] work specs={n}")
    for k,v in c.most_common(): print(f"  {k:13} {v:5} {100*v/n:5.1f}%")
    ac=sum(1 for s in ss if criteria(s.get('description')))
    print(f"  with parseable acceptance criteria: {ac}/{n} = {100*ac/n:.1f}%")
    types=collections.Counter(str(s.get('req_type')) for s in ss)
    print("  by type:",dict(types.most_common()))
    return ss
print("total spec objects:",len(specs))
report('all-time',specs)
report('created <=90d',window(90))
r30=report('created <=30d',window(30))
# top tags in 90d
tc=collections.Counter()
for s in window(90):
    for t in s.get('tags') or []:
        t=str(t)
        if re.match(r'^(parent|followup|from-spike|from-scan|batch|docs|deferred|from-|parent-adr)',t): continue
        tc[t]+=1
print("\n[created <=90d] top 30 tags:",tc.most_common(30))
if ARGS.list30:
    for s in sorted(window(30),key=lambda s:str(s.get('created_at'))):
        if str(s.get('req_type')) in TYPES_WORK: print(layer(s),'|',s['spec_id'],'|',s['title'][:100])

# ---- machine-filed vs authored split, and per-thesis counts ----
def machine(s):
    t=s.get('title') or ''; tags=[str(x) for x in s.get('tags') or []]
    return bool(re.match(r'^(auto-complete failure|Review PR-\d+)',t)) or 'auto-drafted' in tags
THESIS={
 'T1/T2 intent index + traceability': (r'^(north-?star|reconstitution|acceptance-criteria|acceptance|traceability|aida:why|aida:trace|trace|criteria|intent|code-to-intent|harvest-gate|compiler|contradiction|exposition|capture)$',
                                       r'\b(acceptance criteri\w*|trace comment|traceab\w*|reconstitut\w*|intent|aida why|contradiction|harvest)\b'),
 'T3 human-governance layer':         (r'^(governance|human|human-in-the-loop|aida:human|decide|decision|advisor|escalation|approval|persona|proxy|oversight)$',
                                       r'\b(advisor|escalat\w*|human|operator|approv\w*|decide|decision|governance|sign-?off|persona)\b'),
 'T4 TUI is the product':             (r'^(tui|aida:tui|redesign)$', r'\btui\b'),
 'T5 orchestrator + merge queue':     (r'^(orchestrator|drain|aida:queue|queue|merge|merge-queue|verdict|reviewer|auto-complete|autonomy|drain-safe|lease|integrate|ci)$',
                                       r'\b(drain|queue|orchestrat\w*|lease|verdict|merge|integrat\w*|auto-?complete|phase \d|reviewer|seat)\b'),
}
def thesis_hits(ss):
    out=collections.Counter()
    for s in ss:
        tags=[str(x).lower() for x in s.get('tags') or []]; title=(s.get('title') or '').lower()
        for k,(trx,ttl) in THESIS.items():
            if any(re.match(trx,t) for t in tags) or re.search(ttl,title): out[k]+=1
    return out
for label,ss in (('all-time',specs),('created <=90d',window(90)),('created <=30d',window(30))):
    w=[s for s in ss if str(s.get('req_type')) in TYPES_WORK]
    m=[s for s in w if machine(s)]; a=[s for s in w if not machine(s)]
    print(f"\n[{label}] machine-filed={len(m)} authored={len(a)}")
    c=collections.Counter(layer(s) for s in a); n=len(a)
    print("  authored by layer:", ", ".join(f"{k} {v} ({100*v/n:.1f}%)" for k,v in c.most_common()))
    ac=sum(1 for s in a if criteria(s.get('description')))
    print(f"  authored with parseable acceptance criteria: {ac}/{n} = {100*ac/n:.1f}%")
    th=thesis_hits(a)
    print("  authored thesis hits (non-exclusive):", ", ".join(f"{k}: {v} ({100*v/n:.1f}%)" for k,v in th.items()))

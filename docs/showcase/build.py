#!/usr/bin/env python3
"""Build the product site and an integrated reference library from repository docs.

Requires Python 3 and pandoc; the resulting site has no runtime dependencies.
trace:EPIC-b6b1278890774ef993d2ebc327862470-002 | ai:codex
"""
from pathlib import Path
import argparse
import collections
import html
import json
import re
import shutil
import subprocess
from urllib.parse import unquote

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
REPO = "https://github.com/joemooney/aida/blob/docs/product-site/"
NAV_ITEMS = [("workflow.html", "The product"), ("autonomy.html", "Autonomy"), ("configure.html", "Configure"), ("trust.html", "Trust"), ("metrics.html", "Metrics"), ("resources.html", "Docs")]


def header(active="", prefix=""):
    links = ''.join(f'<a href="{prefix}{url}"{chr(32) + "aria-current=\"page\"" if active == url else ""}>{label}</a>' for url, label in NAV_ITEMS)
    return f'''<a class="skip" href="#main">Skip to content</a>
<header class="nav"><a class="brand" href="{prefix}index.html" aria-label="AIDA home"><span class="brand-symbol" aria-hidden="true">✳</span>AIDA<span class="brand-caption">THE GOVERNED SOFTWARE FACTORY</span></a>
<button class="menu-toggle" type="button" aria-expanded="false" aria-controls="site-nav">Menu <span aria-hidden="true">＋</span></button><nav id="site-nav" aria-label="Main navigation">{links}</nav><a class="nav-cta" href="{prefix}start.html">Start a conversation <span aria-hidden="true">↗</span></a></header>'''


def footer(prefix=""):
    return f'''<footer class="shell site-footer"><div><a class="brand" href="{prefix}index.html"><span class="brand-symbol" aria-hidden="true">✳</span>AIDA</a><p>People set the intent.<br>Agents operate the factory.</p></div><div><span class="mono">EXPLORE</span><a href="{prefix}workflow.html">The product</a><a href="{prefix}configure.html">Configurability</a><a href="{prefix}autonomy.html">Selectable autonomy</a></div><div><span class="mono">UNDERSTAND</span><a href="{prefix}trust.html">Trust &amp; boundaries</a><a href="{prefix}metrics.html">Metrics &amp; evidence</a><a href="{prefix}resources.html">Documentation</a><a href="{prefix}executive-brief.html">Executive brief ↗</a></div><div><span class="mono">BUILD</span><a href="{prefix}start.html">Getting started</a><a href="https://github.com/joemooney/aida" target="_blank" rel="noopener">Source repository ↗</a><span class="footer-note">Alpha · Linux only · MIT</span></div></footer>'''


def page(title, description, body, active="", prefix="", extra=""):
    return f'''<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><meta name="theme-color" content="#f4f1e9"><meta name="description" content="{html.escape(description, quote=True)}"><title>{html.escape(title)} — AIDA</title><link rel="stylesheet" href="{prefix}styles.css">{extra}<script src="{prefix}app.js" defer></script></head><body>{header(active,prefix)}<main id="main">{body}</main>{footer(prefix)}<!-- trace:EPIC-b6b1278890774ef993d2ebc327862470-002 | ai:codex --></body></html>'''


def svg_diagram(title, subtitle, nodes, edges, width=1000, height=360):
    """Original accessible diagrams, using fixed reviewed layouts rather than an external renderer."""
    out = [f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width} {height}" role="img" aria-labelledby="title desc"><title id="title">{html.escape(title)}</title><desc id="desc">{html.escape(subtitle)}</desc><defs><pattern id="dots" width="22" height="22" patternUnits="userSpaceOnUse"><circle cx="1" cy="1" r=".8" fill="#c7c3b7"/></pattern><marker id="arrow" markerWidth="6" markerHeight="6" refX="5" refY="3" orient="auto"><path d="M0 0L6 3L0 6" fill="none" stroke="#8b9185"/></marker></defs><rect width="100%" height="100%" fill="#eeebe2"/><rect width="100%" height="100%" fill="url(#dots)"/>']
    for x1, y1, x2, y2, dashed in edges:
        out.append(f'<path d="M{x1} {y1} L{x2} {y2}" stroke="#8b9185" stroke-width="1.5" fill="none" marker-end="url(#arrow)" {"stroke-dasharray=\"5 5\"" if dashed else ""}/>')
    for x,y,w,h,label,detail,kind in nodes:
        fill,stroke,text = {'orange':('#ba3916','#ba3916','#fff8ed'),'dark':('#252a26','#252a26','#fff8ed'),'light':('#f4f1e9','#a8ad9f','#252a26')}[kind]
        out.append(f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="3" fill="{fill}" stroke="{stroke}"/><text x="{x+w/2}" y="{y+h/2-3}" fill="{text}" text-anchor="middle" font-family="Arial,sans-serif" font-size="16">{html.escape(label)}</text><text x="{x+w/2}" y="{y+h/2+20}" fill="{text}" text-anchor="middle" font-family="monospace" font-size="10">{html.escape(detail)}</text>')
    out.append('</svg>')
    return ''.join(out)


def reference_files():
    excluded = {'archive','plans','research','spikes','release-notes','analytics','casts','monitor-contract-fixtures','testing','demos','conformance','plain','showcase','presentation','competitive-analysis'}
    result = []
    for p in sorted((ROOT/'docs').rglob('*.md')):
        rel = p.relative_to(ROOT/'docs')
        if any(part in excluded for part in rel.parts[:-1]) or '/05-decisions/' in p.as_posix():
            continue
        if re.match(r'\d{4}-\d{2}-\d{2}',p.name) or p.name=='review-report.md':
            continue
        result.append(p)
    return result


def reference_name(path):
    return path.relative_to(ROOT/'docs').as_posix().replace('/','--').removesuffix('.md')+'.html'


def rewrite_links(content, source, mapping, output):
    def replace(match):
        attr, quote, url = match.groups()
        if url.startswith(('https:','http:','mailto:','data:','#','/')):
            return match.group(0)
        relative, separator, anchor = url.partition('#')
        target = (source.parent/unquote(relative)).resolve()
        if target.is_relative_to(HERE) and target.suffix == '.html':
            dest = '../'+target.relative_to(HERE).as_posix()+('#'+anchor if separator else '')
        elif target.is_dir() and (target/'README.md') in mapping:
            dest = mapping[target/'README.md']+('#'+anchor if separator else '')
        elif target in mapping:
            dest = mapping[target] + ('#'+anchor if separator else '')
        elif attr=='src' and target.is_file() and target.suffix.lower() in {'.svg','.png','.jpg','.jpeg','.webp','.gif'}:
            rel = target.relative_to(ROOT)
            dest = '../reference-assets/'+rel.as_posix()
            artifact = output/'reference-assets'/rel
            artifact.parent.mkdir(parents=True,exist_ok=True)
            shutil.copy2(target,artifact)
        else:
            try: dest=REPO+target.relative_to(ROOT).as_posix()+('#'+anchor if separator else '')
            except ValueError: dest=url
        return f'{attr}={quote}{html.escape(dest,quote=True)}{quote}'
    return re.sub(r'(href|src)=(["\'])(.*?)\2',replace,content)


def build_reference(output):
    entries=[]; paths=reference_files(); mapping={p.resolve():reference_name(p) for p in paths}
    ref=output/'handbook';ref.mkdir(parents=True,exist_ok=True)
    for path in paths:
        source=path.read_text()
        headings=re.findall(r'^#\s+(.+)$',source,re.M)
        title=re.sub(r'[*`]', '', headings[0]) if headings else path.stem
        category = 'Start here' if path.name in {'WHY-AIDA.md','what-is-aida.md','getting-started.md','first-project.md','configuration.md'} else 'Agent & operator reference' if path.relative_to(ROOT/'docs').parts[0] in {'cli','agents','security','api'} else 'Concepts & architecture'
        rendered=subprocess.run(['pandoc','--from=gfm','--to=html5','--wrap=none','--toc','--template='+str(HERE/'reference-body.html')],input=source,text=True,capture_output=True,check=True).stdout
        rendered=rewrite_links(rendered,path,mapping,output)
        body=f'<div class="reference-shell shell"><aside class="reference-sidebar"><a href="../resources.html">← Documentation library</a><p class="mono">{html.escape(category)}</p><p>This page is rendered from the repository documentation. CLI examples primarily describe agent and operator actions.</p><a href="{REPO+path.relative_to(ROOT).as_posix()}">View source ↗</a><a href="../trust.html">Product boundaries ↗</a></aside><article class="reference-article">{rendered}</article></div>'
        (ref/mapping[path.resolve()]).write_text(page(title,'AIDA documentation: '+title,body,'resources.html','../'))
        stripped=re.sub(r'<!--.*?-->','',source,flags=re.S)
        intro=' '.join(line.strip() for line in stripped.splitlines() if line.strip() and not line.startswith(('#','|','```','>','- ','* ')))[:220]
        keywords=' '.join(sorted(set(re.findall(r'[a-z0-9_-]{3,}',source.lower()))))
        entries.append({'title':title,'category':category,'url':'handbook/'+mapping[path.resolve()],'summary':intro,'source':path.relative_to(ROOT).as_posix(),'keywords':keywords})
    (output/'library-data.js').write_text('window.AIDA_LIBRARY = '+json.dumps(entries,ensure_ascii=False)+';\n')
    return len(entries)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output',type=Path,default=HERE)
    args=parser.parse_args();out=args.output.resolve();out.mkdir(parents=True,exist_ok=True)
    if shutil.which('pandoc') is None: raise SystemExit('pandoc is required to render the integrated reference library')
    for name in ['styles.css','app.js','brief.js','executive-brief.html']:
        if out!=HERE:shutil.copy2(HERE/name,out/name)
    for source in sorted((HERE/'pages').glob('*.html')):
        body=source.read_text();meta=json.loads(body.split('\n',1)[0].removeprefix('<!-- ').removesuffix(' -->'));body=body.split('\n',1)[1]
        if '<!-- AUTONOMY_SELECTOR -->' in body:
            home_source=(HERE/'index.html').read_text()
            selector=re.search(r'<section class="autonomy shell".*?</section>',home_source,re.S).group(0)
            body=body.replace('<!-- AUTONOMY_SELECTOR -->',selector)
        extra='<script src="library-data.js" defer></script>' if source.stem=='resources' else ''
        (out/source.name).write_text(page(meta['title'],meta['description'],body,source.name,extra=extra))
    # The original authored home page remains a portable source, with shared navigation.
    home=(HERE/'index.html').read_text()
    home=re.sub(r'<a class="skip".*?</header>',header(),home,count=1,flags=re.S)
    home=re.sub(r'<footer\b.*?</footer>',footer(),home,count=1,flags=re.S)
    (out/'index.html').write_text(home)
    diagrams={
      'conversation.svg':svg_diagram('People communicate intent; agents operate the factory','A human request passes to an advisor, through agent execution and independent review, then returns as evidence.',[(35,55,210,90,'Your intent','outcomes / decisions','orange'),(330,55,280,90,'AIDA-aware advisor','capture / clarify / route','light'),(695,55,270,90,'Agent delivery','implement / test / review','dark'),(330,235,280,80,'Project record','specs / decisions / history','light'),(695,235,270,80,'Evidence returned','change / verdict / result','orange')],[(245,100,328,100,False),(610,100,693,100,False),(470,145,470,232,True),(830,145,830,232,False),(695,275,612,275,True)],height=360),
      'configuration.svg':svg_diagram('Configure the operating model','Six configurable layers surround the durable project graph; trust scopes remain distinct.',[(350,140,300,90,'Your operating model','durable intent + authority','orange'),(25,35,250,75,'Agent execution','vendor / model / launch','light'),(365,20,270,75,'Work protocols','type / lane / acceptance','light'),(725,35,250,75,'Delivery autonomy','mode per specification','light'),(25,260,250,75,'Workflow policy','queue / review / integrate','light'),(365,280,270,65,'Project footprint','memory / full / shared store','light'),(725,260,250,75,'Reporting','formats / layouts / views','light')],[(275,75,350,160,True),(500,95,500,138,True),(725,75,650,160,True),(275,295,350,210,True),(500,280,500,233,True),(725,295,650,210,True)],height=380),
      'trust.svg':svg_diagram('Autonomy and authority are separate','Human authorization and spec readiness feed execution; review and CI gate the resulting merge.',[(35,45,260,80,'Human authorization','grants / mandates / limits','orange'),(35,220,260,80,'Spec readiness','criteria / decisions / mode','light'),(365,130,260,90,'Agent execution','within the selected contract','dark'),(700,45,260,80,'Independent review','verdict at exact commit','light'),(700,220,260,80,'Merge gates','CI / holds / authority','orange')],[(295,85,363,155,True),(295,260,363,195,True),(625,155,698,85,False),(830,125,830,218,False)],height=350),
      'storage.svg':svg_diagram('A project-owned record with distinct operational layers','The canonical Git-backed spec graph is separate from rebuildable cache and local operational evidence.',[(45,65,270,90,'Code repository','source + code traces','light'),(365,65,270,90,'Git-backed spec graph','canonical project intent','orange'),(685,65,270,90,'Agent interfaces','CLI primary / MCP typed','dark'),(365,235,270,80,'Read projection','rebuildable SQLite cache','light'),(685,235,270,80,'Operational evidence','local review / dispatch state','light')],[(315,110,363,110,True),(635,110,683,110,False),(500,155,500,233,True),(820,155,820,233,True)],height=360),
    }
    diagrams['measurement.svg']=svg_diagram('From intent and implementation to management evidence','Requirements, execution and review records feed reports; people and agents use them to investigate flow, traceability and reliability.',[(35,35,250,75,'Intent record','requirements / decisions','orange'),(35,140,250,75,'Execution evidence','history / usage / CI','light'),(35,245,250,75,'Review evidence','verdicts / findings','light'),(375,125,250,95,'Scoped reports','coverage stays visible','dark'),(715,125,250,95,'People + agents','investigate / decide / improve','orange')],[(285,72,373,150,True),(285,177,373,172,True),(285,282,373,197,True),(625,172,713,172,False)],height=355)
    assets=out/'assets';assets.mkdir(exist_ok=True)
    for name,body in diagrams.items():(assets/name).write_text(body)
    count=build_reference(out)
    print(f'Built product site with {count} integrated reference pages in {out}')


if __name__=='__main__':main()

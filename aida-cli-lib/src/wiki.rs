//! Living project wiki static projection and local private loopback server (EPIC-72, TASK-1439).
//
// trace:EPIC-72 trace:TASK-1439 | ai:antigravity

use crate::cli::WikiCommand;
use crate::exposition::{
    build_bounded_closure, extract_offline_exposition, load_exposition, ExpositionAudience,
};
use anyhow::Result;
use colored::Colorize;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};

/// Default `aida wiki serve` port, registered as `aida-wiki` in the operator's
/// global port registry (`~/.ports`).
// trace:TASK-1470 | ai:claude
pub const DEFAULT_WIKI_PORT: u16 = 8420;

/// File name of the vendored mermaid bundle, written next to the wiki pages and
/// referenced by a relative `<script src>` so no page loads code from a CDN.
pub const MERMAID_ASSET_NAME: &str = "mermaid.min.js";

/// Vendored mermaid 11.6.0 (`dist/mermaid.min.js` from the npm tarball
/// `mermaid-11.6.0.tgz`, MIT, see `assets/wiki/MERMAID-LICENSE`;
/// sha256 3a93016a73dc82ba890d919f9bbb176f3da9d98341650c0b517f2595cc68fef8).
// trace:TASK-1470 | ai:claude
pub const MERMAID_JS: &[u8] = include_bytes!("../assets/wiki/mermaid.min.js");

/// Script tags every wiki page uses to load the LOCAL mermaid bundle.
const MERMAID_SCRIPT_TAGS: &str = r#"<script src="mermaid.min.js"></script>
  <script>mermaid.initialize({ startOnLoad: true, theme: 'dark', securityLevel: 'strict' });</script>"#;

/// Handles the `aida wiki` CLI command.
// trace:TASK-1439 | ai:antigravity
pub fn handle_wiki_command(cmd: &WikiCommand) -> Result<()> {
    match cmd {
        WikiCommand::Build { out } => {
            let out_path = build_wiki(out.as_deref())?;
            println!(
                "{} Built living wiki at {}",
                "SUCCESS:".green().bold(),
                out_path.display()
            );
            Ok(())
        }
        WikiCommand::Serve { port, dir } => serve_wiki(*port, dir.as_deref()),
    }
}

/// Builds the static HTML living wiki projection over canonical specs and exposition sidecars.
// trace:TASK-1439 | ai:antigravity
pub fn build_wiki(out_dir: Option<&Path>) -> Result<PathBuf> {
    let project_root =
        crate::find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());

    let store = crate::load_store_for_lookup(&project_root).ok_or_else(|| {
        anyhow::anyhow!(
            "no requirement store reachable from {} — run where the store is attached",
            project_root.display()
        )
    })?;

    let wiki_dir = out_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| project_root.join(".aida").join("wiki"));

    std::fs::create_dir_all(&wiki_dir)?;

    // 1. Write style.css and the vendored mermaid bundle (no CDN).
    let css = generate_css();
    std::fs::write(wiki_dir.join("style.css"), css)?;
    // trace:TASK-1470 | ai:claude
    std::fs::write(wiki_dir.join(MERMAID_ASSET_NAME), MERMAID_JS)?;

    // 2. Generate pages for each requirement
    let mut spec_rows = Vec::new();

    for req in &store.requirements {
        let spec_id = req.display_id();
        let closure = build_bounded_closure(req, &store.requirements);
        let current_closure_hash = closure.compute_sha256();

        // Load or extract default exposition for operator
        let sidecar = load_exposition(&project_root, &spec_id, ExpositionAudience::Operator)
            .unwrap_or_else(|| {
                extract_offline_exposition(req, ExpositionAudience::Operator, &closure)
            });

        let is_stale = sidecar.is_stale(&current_closure_hash);

        // Record for index table
        spec_rows.push((req.clone(), sidecar.clone(), is_stale));

        // Generate individual spec HTML
        let spec_html = generate_spec_html(req, &sidecar, is_stale, &closure);
        std::fs::write(wiki_dir.join(format!("{spec_id}.html")), spec_html)?;
    }

    // 3. Generate index.html
    let index_html = generate_index_html(&spec_rows);
    std::fs::write(wiki_dir.join("index.html"), index_html)?;

    Ok(wiki_dir)
}

/// The only address `aida wiki serve` binds: IPv4 loopback. There is no host
/// override; the wiki is a private, local-only projection.
// trace:TASK-1470 | ai:claude
pub fn wiki_bind_addr(port: u16) -> SocketAddr {
    SocketAddr::from((Ipv4Addr::LOCALHOST, port))
}

/// Binds the wiki listener on loopback. Refuses (fails closed) if the resulting
/// socket is somehow not a loopback address.
// trace:TASK-1470 | ai:claude
pub fn bind_wiki_listener(port: u16) -> Result<TcpListener> {
    let addr = wiki_bind_addr(port);
    let listener = TcpListener::bind(addr).map_err(|e| {
        anyhow::anyhow!(
            "failed to bind the wiki server to {addr}: {e} (pick a free port with --port)"
        )
    })?;
    let local = listener.local_addr()?;
    if !local.ip().is_loopback() {
        anyhow::bail!("refusing to serve the wiki on non-loopback address {local}");
    }
    Ok(listener)
}

/// Serves the living wiki on private loopback (127.0.0.1).
// trace:TASK-1439 trace:TASK-1470 | ai:claude
pub fn serve_wiki(port: u16, dir: Option<&Path>) -> Result<()> {
    let project_root =
        crate::find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());

    let wiki_dir = dir
        .map(PathBuf::from)
        .unwrap_or_else(|| project_root.join(".aida").join("wiki"));

    if !wiki_dir.join("index.html").exists() {
        println!("Wiki not found. Building living wiki first...");
        build_wiki(Some(&wiki_dir))?;
    }

    let listener = bind_wiki_listener(port)?;
    let bound = listener.local_addr()?;

    println!();
    println!(
        "{} AIDA Living Wiki serving on {}",
        "ONLINE:".green().bold(),
        format!("http://{bound}").cyan().underline()
    );
    println!("Loopback only: reachable from this machine, not the network.");
    println!("Serving from: {}", wiki_dir.display());
    println!("Press Ctrl+C to terminate.");
    println!();

    serve_connections(&listener, &wiki_dir, None);
    Ok(())
}

/// Accept loop. `max_connections` bounds the loop (tests); `None` serves forever.
// trace:TASK-1470 | ai:claude
pub fn serve_connections(listener: &TcpListener, wiki_dir: &Path, max_connections: Option<usize>) {
    for (index, stream_res) in listener.incoming().enumerate() {
        match stream_res {
            Ok(mut stream) => handle_connection(&mut stream, wiki_dir),
            Err(e) => eprintln!("Connection error: {e}"),
        }
        if max_connections.is_some_and(|max| index + 1 >= max) {
            break;
        }
    }
}

fn handle_connection(stream: &mut TcpStream, wiki_dir: &Path) {
    // A stalled client must not wedge the single-threaded server.
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    // Read until the end of the request headers (bounded), since a request
    // line can arrive split across several TCP segments.
    let mut buf = [0u8; 8192];
    let mut n = 0usize;
    while n < buf.len() {
        match stream.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(read) => {
                n += read;
                if buf[..n].windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    if n == 0 {
        return;
    }
    let req_str = String::from_utf8_lossy(&buf[..n]);
    let Some(req_line) = req_str.lines().next() else {
        return;
    };
    let parts: Vec<&str> = req_line.split_whitespace().collect();
    if parts.len() < 2 || parts[0] != "GET" {
        write_simple(stream, "405 Method Not Allowed", "Method Not Allowed");
        return;
    }
    let raw_path = parts[1].split('?').next().unwrap_or("");
    let clean_path = raw_path.trim_start_matches('/');
    let file_rel = if clean_path.is_empty() {
        "index.html"
    } else {
        clean_path
    };

    // Security: prevent directory traversal.
    if file_rel.contains("..") || file_rel.starts_with('/') || file_rel.contains('\\') {
        write_simple(stream, "403 Forbidden", "Forbidden");
        return;
    }

    let file_path = wiki_dir.join(file_rel);
    let contents = if file_path.is_file() {
        std::fs::read(&file_path).ok()
    } else if file_rel == MERMAID_ASSET_NAME {
        // A wiki built before mermaid was vendored still renders diagrams:
        // serve the embedded bundle rather than reaching for a CDN.
        Some(MERMAID_JS.to_vec())
    } else {
        None
    };
    let Some(contents) = contents else {
        write_simple(stream, "404 Not Found", "404 Not Found");
        return;
    };
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n",
        content_type_for(file_rel),
        contents.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(&contents);
}

fn write_simple(stream: &mut TcpStream, status: &str, body: &str) {
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(resp.as_bytes());
}

pub(crate) fn content_type_for(file_rel: &str) -> &'static str {
    if file_rel.ends_with(".html") {
        "text/html; charset=utf-8"
    } else if file_rel.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if file_rel.ends_with(".js") {
        "application/javascript; charset=utf-8"
    } else if file_rel.ends_with(".json") {
        "application/json; charset=utf-8"
    } else {
        "application/octet-stream"
    }
}

fn generate_css() -> &'static str {
    r#"
:root {
  --bg-primary: #0f172a;
  --bg-secondary: #1e293b;
  --bg-card: #334155;
  --text-main: #f8fafc;
  --text-muted: #94a3b8;
  --border: #475569;
  --accent: #38bdf8;
  --accent-hover: #0284c7;
  --success: #10b981;
  --warning: #f59e0b;
  --danger: #ef4444;
  --purple: #a855f7;
}

* { box-sizing: border-box; margin: 0; padding: 0; }
body {
  font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
  background-color: var(--bg-primary);
  color: var(--text-main);
  line-height: 1.6;
  padding: 2rem;
}
.container { max-width: 1200px; margin: 0 auto; }
header { margin-bottom: 2rem; border-bottom: 1px solid var(--border); padding-bottom: 1rem; }
header h1 { font-size: 2.2rem; color: var(--accent); margin-bottom: 0.5rem; }
header p { color: var(--text-muted); }
.nav-back { display: inline-block; margin-bottom: 1.5rem; color: var(--accent); text-decoration: none; font-weight: 500; }
.nav-back:hover { text-decoration: underline; }

.badge { display: inline-block; padding: 0.25rem 0.6rem; border-radius: 9999px; font-size: 0.75rem; font-weight: 600; text-transform: uppercase; }
.badge-fresh { background-color: rgba(16, 185, 129, 0.2); color: var(--success); border: 1px solid var(--success); }
.badge-stale { background-color: rgba(239, 68, 68, 0.2); color: var(--danger); border: 1px solid var(--danger); }
.badge-reviewed { background-color: rgba(168, 85, 247, 0.2); color: var(--purple); border: 1px solid var(--purple); }
.badge-type { background-color: var(--bg-card); color: var(--text-main); }
.badge-status { background-color: rgba(56, 189, 248, 0.2); color: var(--accent); }

.grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(360px, 1fr)); gap: 1.5rem; margin-top: 1.5rem; }
.card { background-color: var(--bg-secondary); border: 1px solid var(--border); border-radius: 8px; padding: 1.5rem; transition: transform 0.15s ease, border-color 0.15s ease; }
.card:hover { border-color: var(--accent); transform: translateY(-2px); }
.card h3 { font-size: 1.25rem; margin-bottom: 0.5rem; }
.card h3 a { color: var(--accent); text-decoration: none; }
.card h3 a:hover { text-decoration: underline; }
.card p { color: var(--text-muted); font-size: 0.95rem; margin-bottom: 1rem; }

.table-wrap { overflow-x: auto; margin-top: 2rem; }
table { width: 100%; border-collapse: collapse; text-align: left; background-color: var(--bg-secondary); border-radius: 8px; overflow: hidden; }
th, td { padding: 0.85rem 1rem; border-bottom: 1px solid var(--border); }
th { background-color: var(--bg-card); color: var(--accent); font-weight: 600; text-transform: uppercase; font-size: 0.8rem; }
tr:hover { background-color: rgba(255, 255, 255, 0.02); }

.exposition-box { background-color: var(--bg-secondary); border-left: 4px solid var(--accent); padding: 1.5rem; border-radius: 0 8px 8px 0; margin-bottom: 2rem; }
.canonical-box { background-color: rgba(30, 41, 59, 0.5); border: 1px dashed var(--border); padding: 1.5rem; border-radius: 8px; margin-bottom: 2rem; }
.section-title { font-size: 1.3rem; margin-top: 1.5rem; margin-bottom: 0.75rem; color: var(--accent); border-bottom: 1px solid var(--border); padding-bottom: 0.3rem; }
.list { margin-left: 1.5rem; margin-bottom: 1rem; }
.list li { margin-bottom: 0.4rem; }

.diagram-box { background-color: #0b1120; border: 1px solid var(--border); border-radius: 8px; padding: 1.5rem; margin: 2rem 0; overflow-x: auto; font-family: monospace; }
.stats-bar { display: flex; gap: 2rem; background-color: var(--bg-secondary); border: 1px solid var(--border); border-radius: 8px; padding: 1rem 1.5rem; margin-bottom: 2rem; }
.stat-item { display: flex; flex-direction: column; }
.stat-val { font-size: 1.8rem; font-weight: 700; color: var(--accent); }
.stat-label { font-size: 0.85rem; color: var(--text-muted); text-transform: uppercase; }

pre { background-color: #0b1120; padding: 1rem; border-radius: 6px; overflow-x: auto; font-size: 0.9rem; color: #cbd5e1; }
code { font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace; }
"#
}

pub(crate) fn generate_index_html(
    rows: &[(
        aida_core::Requirement,
        crate::exposition::ExpositionSidecar,
        bool,
    )],
) -> String {
    let total = rows.len();
    let fresh_count = rows.iter().filter(|(_, _, stale)| !*stale).count();
    let stale_count = total - fresh_count;
    let reviewed_count = rows
        .iter()
        .filter(|(_, sidecar, _)| sidecar.is_human_reviewed())
        .count();

    let architecture_map = generate_architecture_map(rows);

    let mut trs = String::new();
    for (req, sidecar, is_stale) in rows {
        let spec_id = req.display_id();
        let fresh_badge = if *is_stale {
            r#"<span class="badge badge-stale">Stale</span>"#
        } else {
            r#"<span class="badge badge-fresh">Fresh</span>"#
        };

        let review_badge = if sidecar.is_human_reviewed() {
            r#"<span class="badge badge-reviewed">Human</span>"#
        } else {
            ""
        };

        let audit_score = sidecar
            .audit
            .as_ref()
            .map(|a| format!("{:.0}%", a.readability_score * 100.0))
            .unwrap_or_else(|| "N/A".to_string());

        trs.push_str(&format!(
            r#"<tr>
  <td><a href="{spec_id}.html" style="color:var(--accent);font-weight:600;">{spec_id}</a></td>
  <td><span class="badge badge-type">{:?}</span></td>
  <td><span class="badge badge-status">{}</span></td>
  <td>{fresh_badge} {review_badge}</td>
  <td><strong>{}</strong><br><span style="color:var(--text-muted);font-size:0.88rem;">{}</span></td>
  <td>{audit_score}</td>
</tr>
"#,
            req.req_type,
            req.status,
            escape_html(&req.title),
            escape_html(sidecar.effective_summary())
        ));
    }

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <title>AIDA Living Project Wiki</title>
  <link rel="stylesheet" href="style.css">
  {MERMAID_SCRIPT_TAGS}
</head>
<body>
  <div class="container">
    <header>
      <h1>AIDA Living Project Wiki</h1>
      <p>Human-centric exposition layer, bounded architecture closure, and living system documentation.</p>
    </header>

    <div class="stats-bar">
      <div class="stat-item"><span class="stat-val">{total}</span><span class="stat-label">Total Specs</span></div>
      <div class="stat-item"><span class="stat-val">{fresh_count}</span><span class="stat-label">Fresh Expositions</span></div>
      <div class="stat-item"><span class="stat-val">{stale_count}</span><span class="stat-label">Stale Drifts</span></div>
      <div class="stat-item"><span class="stat-val">{reviewed_count}</span><span class="stat-label">Human Protected</span></div>
    </div>

    <div class="section-title">High-Level Architecture Map</div>
    <div class="diagram-box">
      <pre class="mermaid">
{architecture_map}
      </pre>
    </div>

    <div class="section-title">Requirements Inventory & Human Summaries</div>
    <div class="table-wrap">
      <table>
        <thead>
          <tr>
            <th>Spec ID</th>
            <th>Type</th>
            <th>Status</th>
            <th>Freshness</th>
            <th>Title & Plain Summary</th>
            <th>Readability</th>
          </tr>
        </thead>
        <tbody>
          {trs}
        </tbody>
      </table>
    </div>
  </div>
</body>
</html>
"#
    )
}

/// Upper bound on epic-to-child edges drawn in the index map, so a large store
/// still renders a readable diagram.
const ARCHITECTURE_MAP_MAX_EDGES: usize = 60;

/// Epic-to-child map derived from the store being projected. The feature
/// branch hard-coded one AIDA epic here, which every project's wiki would have
/// shown; this draws the project's own epics instead. Labels are spec IDs only
/// (never free text), so nothing user-authored reaches the mermaid source.
// trace:TASK-1470 | ai:claude
pub(crate) fn generate_architecture_map(
    rows: &[(
        aida_core::Requirement,
        crate::exposition::ExpositionSidecar,
        bool,
    )],
) -> String {
    use aida_core::{RelationshipType, RequirementType};
    let node = |id: &str| -> String {
        id.chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect()
    };
    let label = |id: &str| -> String { id.replace('"', "") };

    let mut edges: Vec<(String, String)> = Vec::new();
    for (req, _, _) in rows {
        for rel in &req.relationships {
            if rel.rel_type != RelationshipType::Parent {
                continue;
            }
            if let Some((parent, _, _)) = rows
                .iter()
                .find(|(r, _, _)| r.id == rel.target_id && r.req_type == RequirementType::Epic)
            {
                edges.push((parent.display_id(), req.display_id()));
            }
        }
    }
    edges.sort();
    edges.dedup();

    let mut out = String::from("graph LR\n");
    if edges.is_empty() {
        out.push_str("  none[\"No epics with child specs yet\"]\n");
        return out;
    }
    let total = edges.len();
    for (parent, child) in edges.iter().take(ARCHITECTURE_MAP_MAX_EDGES) {
        out.push_str(&format!(
            "  {}[\"{}\"] --> {}[\"{}\"]\n",
            node(parent),
            label(parent),
            node(child),
            label(child)
        ));
    }
    if total > ARCHITECTURE_MAP_MAX_EDGES {
        out.push_str(&format!(
            "  more[\"... {} more links\"]\n",
            total - ARCHITECTURE_MAP_MAX_EDGES
        ));
    }
    out
}

pub(crate) fn generate_spec_html(
    req: &aida_core::Requirement,
    sidecar: &crate::exposition::ExpositionSidecar,
    is_stale: bool,
    closure: &crate::exposition::BoundedClosureInputs,
) -> String {
    let spec_id = req.display_id();
    let fresh_badge = if is_stale {
        r#"<span class="badge badge-stale">Stale (Graph Closure Drifted)</span>"#
    } else {
        r#"<span class="badge badge-fresh">Fresh (Synchronized)</span>"#
    };

    let review_badge = if sidecar.is_human_reviewed() {
        r#"<span class="badge badge-reviewed">Human Reviewed</span>"#
    } else {
        ""
    };

    let mut constraints_li = String::new();
    for c in &sidecar.key_constraints {
        constraints_li.push_str(&format!("<li>{}</li>", escape_html(c)));
    }

    let mut tradeoffs_li = String::new();
    for t in &sidecar.tradeoffs {
        tradeoffs_li.push_str(&format!("<li>{}</li>", escape_html(t)));
    }

    let mut questions_li = String::new();
    for q in &sidecar.open_questions {
        questions_li.push_str(&format!("<li>{}</li>", escape_html(q)));
    }

    // Generate local Mermaid graph
    let mut mermaid_nodes = String::new();
    let safe_current = spec_id.replace('-', "_");
    mermaid_nodes.push_str(&format!(
        "  {safe_current}[\"{spec_id} (Current)\"]\n  style {safe_current} fill:#0284c7,stroke:#38bdf8,stroke-width:2px,color:#fff\n"
    ));

    for neighbor in &closure.neighbors {
        let safe_n = neighbor.id.replace('-', "_");
        let label = format!("{}: {}", neighbor.relationship, neighbor.id);
        if neighbor.relationship == "Parent" {
            mermaid_nodes.push_str(&format!("  {safe_n}[\"{label}\"] --> {safe_current}\n"));
        } else if neighbor.relationship == "Blocks" {
            mermaid_nodes.push_str(&format!(
                "  {safe_current} -->|Blocks| {safe_n}[\"{label}\"]\n"
            ));
        } else if neighbor.relationship == "BlockedBy" {
            mermaid_nodes.push_str(&format!(
                "  {safe_n}[\"{label}\"] -->|Blocks| {safe_current}\n"
            ));
        } else {
            mermaid_nodes.push_str(&format!("  {safe_current} -.-> {safe_n}[\"{label}\"]\n"));
        }
    }

    let audit_box = if let Some(audit) = &sidecar.audit {
        let verdict = if audit.needs_revision {
            r#"<span style="color:var(--danger);font-weight:bold;">Needs Revision</span>"#
        } else {
            r#"<span style="color:var(--success);font-weight:bold;">Passed</span>"#
        };
        let mut findings_html = String::new();
        for f in &audit.findings {
            findings_html.push_str(&format!("<li>{}</li>", escape_html(f)));
        }
        format!(
            r#"<div style="background-color:var(--bg-card);padding:1rem;border-radius:6px;margin-top:1rem;">
  <strong>Quality Audit ({})</strong>: Readability: {:.0}%, Jargon: {:.0}%, Constraint Preservation: {:.0}% — Status: {}
  <ul class="list" style="margin-top:0.5rem;">{}</ul>
</div>"#,
            audit.evaluator,
            audit.readability_score * 100.0,
            audit.jargon_saturation_score * 100.0,
            audit.constraint_preservation_score * 100.0,
            verdict,
            findings_html
        )
    } else {
        String::new()
    };

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <title>{spec_id} - AIDA Living Wiki</title>
  <link rel="stylesheet" href="style.css">
  {MERMAID_SCRIPT_TAGS}
</head>
<body>
  <div class="container">
    <a href="index.html" class="nav-back">← Back to Overview Wiki</a>
    <header>
      <h1>{spec_id}: {}</h1>
      <div>
        <span class="badge badge-type">{:?}</span>
        <span class="badge badge-status">{}</span>
        {fresh_badge}
        {review_badge}
      </div>
    </header>

    <div class="exposition-box">
      <div class="section-title">Human-Centric Plain Explanation</div>
      <p style="font-size:1.1rem;margin-bottom:1rem;"><strong>Summary:</strong> {}</p>
      <p style="margin-bottom:1rem;"><strong>Rationale:</strong> {}</p>

      <strong>Key Constraints & Acceptance Rules:</strong>
      <ul class="list">{constraints_li}</ul>

      <strong>Trade-offs & Alternatives:</strong>
      <ul class="list">{tradeoffs_li}</ul>

      <strong>Open Questions:</strong>
      <ul class="list">{questions_li}</ul>

      {audit_box}
    </div>

    <div class="section-title">Immediate Bounded Neighborhood (Mermaid Graph)</div>
    <div class="diagram-box">
      <pre class="mermaid">
graph TD
{mermaid_nodes}
      </pre>
    </div>

    <div class="canonical-box">
      <div class="section-title">Canonical Specification (Machine Authoritative)</div>
      <pre><code>{}</code></pre>
    </div>
  </div>
</body>
</html>
"#,
        escape_html(&req.title),
        req.req_type,
        req.status,
        escape_html(sidecar.effective_summary()),
        escape_html(&sidecar.rationale),
        escape_html(&req.description)
    )
}

fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

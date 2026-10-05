// trace:TASK-1538 | ai:claude
//! Structural tooling for the STORY-1488 lib.rs split (slice 1).
//!
//! Subcommands (all read-only; text surgery happens in the Python driver):
//!   spans <file.rs>       TSV of every top-level item: start, end, kind, name, vis
//!   visibility <file.rs>  TSV of `pub(crate) ` insertion points (line, col) per
//!                         STORY-1488 amendment 2: private top-level items, their
//!                         named fields, and methods of top-level INHERENT impls.
//!                         Trait impls are skipped structurally (E0449), and items
//!                         inside `mod { ... }` bodies are never visited.
//!   tokens <file.rs>      Flattened token stream, one per line, for the V1
//!                         token-identity proof (checker drops `pub ( crate )`).

use proc_macro2::TokenTree;
use std::fmt::Write as _;
use syn::spanned::Spanned;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: itemtool <spans|visibility|tokens> <file.rs>");
        std::process::exit(2);
    }
    let src = std::fs::read_to_string(&args[2]).expect("read source file");
    let file = syn::parse_file(&src).expect("parse source file");
    match args[1].as_str() {
        "spans" => spans(&file),
        "visibility" => visibility(&file),
        "tokens" => tokens(&file),
        other => {
            eprintln!("unknown subcommand: {other}");
            std::process::exit(2);
        }
    }
}

fn flat<T: quote::ToTokens>(ts: &T) -> String {
    let mut s = ts
        .to_token_stream()
        .to_string()
        .replace(' ', "")
        .replace('\n', "");
    s.truncate(120);
    s
}

fn vis_str(vis: &syn::Visibility) -> &'static str {
    match vis {
        syn::Visibility::Public(_) => "pub",
        syn::Visibility::Restricted(_) => "pub(restricted)",
        syn::Visibility::Inherited => "private",
    }
}

fn item_facts(item: &syn::Item) -> (String, String, &'static str) {
    use syn::Item::*;
    match item {
        Fn(i) => ("fn".into(), i.sig.ident.to_string(), vis_str(&i.vis)),
        Struct(i) => ("struct".into(), i.ident.to_string(), vis_str(&i.vis)),
        Enum(i) => ("enum".into(), i.ident.to_string(), vis_str(&i.vis)),
        Trait(i) => ("trait".into(), i.ident.to_string(), vis_str(&i.vis)),
        Type(i) => ("type".into(), i.ident.to_string(), vis_str(&i.vis)),
        Const(i) => ("const".into(), i.ident.to_string(), vis_str(&i.vis)),
        Static(i) => ("static".into(), i.ident.to_string(), vis_str(&i.vis)),
        Union(i) => ("union".into(), i.ident.to_string(), vis_str(&i.vis)),
        Mod(i) => ("mod".into(), i.ident.to_string(), vis_str(&i.vis)),
        Use(i) => ("use".into(), flat(&i.tree), vis_str(&i.vis)),
        Impl(i) => {
            let name = match &i.trait_ {
                Some((_, path, _)) => format!("impl:{}:for:{}", flat(path), flat(&i.self_ty)),
                None => format!("impl:{}", flat(&i.self_ty)),
            };
            ("impl".into(), name, "n/a")
        }
        Macro(i) => {
            let mut name = format!("macro:{}", flat(&i.mac.path));
            if let Some(id) = &i.ident {
                let _ = write!(name, ":{id}");
            }
            (name, String::new(), "n/a")
        }
        ExternCrate(i) => ("extern_crate".into(), i.ident.to_string(), vis_str(&i.vis)),
        ForeignMod(_) => ("foreign_mod".into(), String::new(), "n/a"),
        TraitAlias(i) => ("trait_alias".into(), i.ident.to_string(), vis_str(&i.vis)),
        _ => ("unknown".into(), String::new(), "n/a"),
    }
}

/// Span of the whole item INCLUDING its outer attributes and doc comments.
/// `Spanned` on the item joins all its tokens, but join can fail outside
/// proc-macro context, so take explicit min/max over attr + item spans.
fn full_lines(item: &syn::Item) -> (usize, usize) {
    let sp = item.span();
    let (mut start, mut end) = (sp.start().line, sp.end().line);
    let attrs: &[syn::Attribute] = match item {
        syn::Item::Fn(i) => &i.attrs,
        syn::Item::Struct(i) => &i.attrs,
        syn::Item::Enum(i) => &i.attrs,
        syn::Item::Trait(i) => &i.attrs,
        syn::Item::Type(i) => &i.attrs,
        syn::Item::Const(i) => &i.attrs,
        syn::Item::Static(i) => &i.attrs,
        syn::Item::Union(i) => &i.attrs,
        syn::Item::Mod(i) => &i.attrs,
        syn::Item::Use(i) => &i.attrs,
        syn::Item::Impl(i) => &i.attrs,
        syn::Item::Macro(i) => &i.attrs,
        syn::Item::ExternCrate(i) => &i.attrs,
        syn::Item::TraitAlias(i) => &i.attrs,
        _ => &[],
    };
    for a in attrs {
        let s = a.span();
        start = start.min(s.start().line);
        end = end.max(s.end().line);
    }
    // Tokens inside the item can only extend the end.
    end = end.max(item.span().end().line);
    (start, end)
}

fn spans(file: &syn::File) {
    println!("start\tend\tkind\tname\tvis");
    for item in &file.items {
        let (kind, name, vis) = item_facts(item);
        let (start, end) = full_lines(item);
        let (kind, name) = if name.is_empty() {
            // ItemMacro packs path into kind slot
            (kind, String::new())
        } else {
            (kind, name)
        };
        println!("{start}\t{end}\t{kind}\t{name}\t{vis}");
    }
}

fn insertion_point(sp: proc_macro2::Span) -> (usize, usize) {
    (sp.start().line, sp.start().column)
}

fn visibility(file: &syn::File) {
    // line \t col \t why   (1-based line, 0-based col; apply bottom-up)
    let mut out: Vec<(usize, usize, String)> = Vec::new();
    for item in &file.items {
        use syn::Item::*;
        match item {
            Fn(i) if matches!(i.vis, syn::Visibility::Inherited) => {
                let p = insertion_point(i.sig.span());
                out.push((p.0, p.1, format!("fn {}", i.sig.ident)));
            }
            Struct(i) => {
                if matches!(i.vis, syn::Visibility::Inherited) {
                    let p = insertion_point(i.struct_token.span());
                    out.push((p.0, p.1, format!("struct {}", i.ident)));
                }
                // Private fields of EVERY top-level struct (whatever the
                // struct's own visibility): a private field at the crate
                // root is visible crate-wide today, but becomes
                // module-private after the move. pub(crate) preserves the
                // status quo exactly.
                match &i.fields {
                    syn::Fields::Named(fields) => {
                        for f in &fields.named {
                            if matches!(f.vis, syn::Visibility::Inherited) {
                                let p = insertion_point(f.ident.as_ref().unwrap().span());
                                out.push((
                                    p.0,
                                    p.1,
                                    format!("field {}.{}", i.ident, f.ident.as_ref().unwrap()),
                                ));
                            }
                        }
                    }
                    syn::Fields::Unnamed(fields) => {
                        for (n, f) in fields.unnamed.iter().enumerate() {
                            if matches!(f.vis, syn::Visibility::Inherited) {
                                // Type span, NOT field span: the field span
                                // includes attributes, and `pub(crate)` must
                                // land after them (finding 5d on PR 2425).
                                let p = insertion_point(f.ty.span());
                                out.push((p.0, p.1, format!("field {}.{}", i.ident, n)));
                            }
                        }
                    }
                    syn::Fields::Unit => {}
                }
            }
            Enum(i) if matches!(i.vis, syn::Visibility::Inherited) => {
                let p = insertion_point(i.enum_token.span());
                out.push((p.0, p.1, format!("enum {}", i.ident)));
            }
            Trait(i) if matches!(i.vis, syn::Visibility::Inherited) => {
                let sp = i
                    .unsafety
                    .map(|t| t.span())
                    .unwrap_or_else(|| i.trait_token.span());
                let p = insertion_point(sp);
                out.push((p.0, p.1, format!("trait {}", i.ident)));
            }
            Type(i) if matches!(i.vis, syn::Visibility::Inherited) => {
                let p = insertion_point(i.type_token.span());
                out.push((p.0, p.1, format!("type {}", i.ident)));
            }
            Const(i) if matches!(i.vis, syn::Visibility::Inherited) => {
                let p = insertion_point(i.const_token.span());
                out.push((p.0, p.1, format!("const {}", i.ident)));
            }
            Static(i) if matches!(i.vis, syn::Visibility::Inherited) => {
                let p = insertion_point(i.static_token.span());
                out.push((p.0, p.1, format!("static {}", i.ident)));
            }
            Union(i) if matches!(i.vis, syn::Visibility::Inherited) => {
                let p = insertion_point(i.union_token.span());
                out.push((p.0, p.1, format!("union {}", i.ident)));
            }
            // Inherent impls only; trait impls are E0449 territory (amendment 2).
            Impl(i) if i.trait_.is_none() => {
                for it in &i.items {
                    if let syn::ImplItem::Fn(m) = it {
                        if matches!(m.vis, syn::Visibility::Inherited) {
                            let p = insertion_point(m.sig.span());
                            out.push((
                                p.0,
                                p.1,
                                format!("method {}::{}", flat(&i.self_ty), m.sig.ident),
                            ));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    println!("line\tcol\twhy");
    for (l, c, w) in out {
        println!("{l}\t{c}\t{w}");
    }
}

fn tokens(file: &syn::File) {
    fn walk(ts: proc_macro2::TokenStream, out: &mut Vec<String>) {
        for tt in ts {
            match tt {
                TokenTree::Group(g) => {
                    let (open, close) = match g.delimiter() {
                        proc_macro2::Delimiter::Parenthesis => ("(", ")"),
                        proc_macro2::Delimiter::Brace => ("{", "}"),
                        proc_macro2::Delimiter::Bracket => ("[", "]"),
                        proc_macro2::Delimiter::None => ("", ""),
                    };
                    if !open.is_empty() {
                        out.push(open.to_string());
                    }
                    walk(g.stream(), out);
                    if !close.is_empty() {
                        out.push(close.to_string());
                    }
                }
                TokenTree::Ident(i) => out.push(i.to_string()),
                TokenTree::Punct(p) => out.push(p.as_char().to_string()),
                TokenTree::Literal(l) => out.push(l.to_string()),
            }
        }
    }
    let mut out = Vec::new();
    walk(quote::ToTokens::to_token_stream(file), &mut out);
    use std::io::Write;
    let stdout = std::io::stdout();
    let mut w = std::io::BufWriter::new(stdout.lock());
    for t in out {
        writeln!(w, "{t}").unwrap();
    }
}

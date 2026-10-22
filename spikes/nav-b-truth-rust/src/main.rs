//! Ground truth for Rust (spike S-B): the items `syn` finds, as
//! `path<TAB>line<TAB>class<TAB>name`, the same rows the tags run dumps.
//! usage: spike-nav-b-truth-rust <root> > truth.tsv

use std::path::Path;
use syn::visit::Visit;

struct Rows<'a> {
    rel: &'a str,
    out: &'a mut Vec<String>,
}

impl Rows<'_> {
    fn add(&mut self, class: &str, ident: &proc_macro2::Ident) {
        self.out.push(format!("{}\t{}\t{}\t{}", self.rel, ident.span().start().line, class, ident));
    }
}

impl<'ast> Visit<'ast> for Rows<'_> {
    fn visit_item_fn(&mut self, i: &'ast syn::ItemFn) {
        self.add("fn", &i.sig.ident);
        syn::visit::visit_item_fn(self, i);
    }
    fn visit_impl_item_fn(&mut self, i: &'ast syn::ImplItemFn) {
        self.add("fn", &i.sig.ident);
        syn::visit::visit_impl_item_fn(self, i);
    }
    fn visit_trait_item_fn(&mut self, i: &'ast syn::TraitItemFn) {
        // A trait method with a body is a definition; one without is a signature.
        if i.default.is_some() {
            self.add("fn", &i.sig.ident);
        }
        syn::visit::visit_trait_item_fn(self, i);
    }
    fn visit_item_struct(&mut self, i: &'ast syn::ItemStruct) {
        self.add("type", &i.ident);
        syn::visit::visit_item_struct(self, i);
    }
    fn visit_item_enum(&mut self, i: &'ast syn::ItemEnum) {
        self.add("type", &i.ident);
        syn::visit::visit_item_enum(self, i);
    }
    fn visit_item_union(&mut self, i: &'ast syn::ItemUnion) {
        self.add("type", &i.ident);
        syn::visit::visit_item_union(self, i);
    }
    fn visit_item_type(&mut self, i: &'ast syn::ItemType) {
        self.add("type", &i.ident);
        syn::visit::visit_item_type(self, i);
    }
    fn visit_item_trait(&mut self, i: &'ast syn::ItemTrait) {
        self.add("type", &i.ident);
        syn::visit::visit_item_trait(self, i);
    }
}

fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let Ok(t) = e.file_type() else { continue };
        let p = e.path();
        if t.is_symlink() {
            continue;
        }
        if t.is_dir() {
            if p.file_name().is_some_and(|n| n == ".git") {
                continue;
            }
            walk(base, &p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            if e.metadata().map_or(true, |m| m.len() > 2 * 1024 * 1024) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&p) else { continue };
            let Ok(file) = syn::parse_file(&text) else { continue };
            let rel = p.strip_prefix(base).unwrap_or(&p).display().to_string();
            Rows { rel: &rel, out }.visit_file(&file);
        }
    }
}

fn main() {
    let root = std::env::args().nth(1).expect("root");
    let mut out = Vec::new();
    walk(Path::new(&root), Path::new(&root), &mut out);
    out.sort();
    println!("{}", out.join("\n"));
}

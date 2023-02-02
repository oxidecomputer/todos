use anyhow::ensure;
use anyhow::Context;
use proc_macro2::LineColumn;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::Path;
use syn::__private::ToTokens;
use syn::spanned::Spanned;
use syn::visit::Visit;

fn main() -> Result<(), anyhow::Error> {
    let argv = std::env::args_os().collect::<Vec<_>>();
    ensure!(argv.len() == 2, "usage: todos path/to/file/tree");

    let mut tracker = CommentTracker::new();
    let walker = walkdir::WalkDir::new(&argv[1])
        .follow_links(false)
        .same_file_system(false)
        .into_iter()
        .filter_entry(|e| {
            // Skip any "target" directory found at the root.
            // TODO-cleanup This looks awful.
            let skip = if e.depth() == 1 {
                if let Some(name) = e.path().file_name() {
                    name == "target"
                } else {
                    false
                }
            } else {
                false
            };
            if skip {
                eprintln!(
                    "skipping {:?} (looks like \"target\" directory)",
                    e.path().display()
                );
            }
            !skip
        });
    for maybe_entry in walker {
        if let Err(error) = do_file(&mut tracker, maybe_entry) {
            eprintln!("warn: {:#}", error);
        }
    }

    for (label, comments) in &tracker.comments_by_kind {
        println!("{} (comments: {})", label, comments.len());
        for c in comments {
            println!("  file {} line {}", c.file, c.location);
            println!(
                "{}",
                c.contents
                    .lines()
                    .map(|l| format!("    {}\n", l))
                    .collect::<Vec<_>>()
                    .join("")
            );
        }
        println!("====");
    }

    Ok(())
}

fn do_file(
    tracker: &mut CommentTracker,
    maybe_entry: Result<walkdir::DirEntry, walkdir::Error>,
) -> Result<(), anyhow::Error> {
    if maybe_entry.is_err() {
        // walkdir failed to read this entry.
        return maybe_entry.map(|_| ()).context("walking tree");
    }

    let entry = maybe_entry.unwrap();
    let path = entry.path();

    // Skip anything that doesn't end with ".rs".
    match path.extension() {
        Some(ext) if ext == "rs" => (),
        _ => {
            return Ok(());
        }
    }

    // Open the file and then stat it (presumably by fd).  Skip anything that's
    // not a regular file.
    let file = std::fs::File::open(entry.path())
        .with_context(|| format!("open {:?}", path.display()))?;
    let metadata = file
        .metadata()
        .with_context(|| format!("metadata for {:?}", path.display()))?;
    if !metadata.is_file() {
        return Ok(());
    }

    println!("reading {:?}", path.display());
    let mut contents = std::io::read_to_string(&file)
        .with_context(|| format!("read {:?}", path.display()))?;
    let parsed = syn::parse_file(&mut contents)
        .with_context(|| format!("parsing {:?}", path.display()))?;
    let mut visitor = TodoVisitor::new(path, tracker);

    visitor.visit_file(&parsed);

    Ok(())
}

struct Comment {
    contents: String,
    file: String,
    location: String,
}

struct CommentTracker {
    comments_by_kind: BTreeMap<String, Vec<Comment>>,
}

impl CommentTracker {
    fn new() -> CommentTracker {
        CommentTracker { comments_by_kind: BTreeMap::new() }
    }

    fn found_possible_comment(
        &mut self,
        contents: &str,
        path: &Path,
        wher: LineColumn,
    ) {
        let mut found_kinds = BTreeSet::new();

        for line in contents.lines() {
            let words = line.split_whitespace();
            for word in words {
                if word.starts_with("XXX")
                    || word.starts_with("FIXME")
                    || word.starts_with("TODO")
                {
                    found_kinds.insert(word);
                }
            }
        }

        for k in found_kinds {
            let comments_for_this_kind = self
                .comments_by_kind
                .entry(k.to_string())
                .or_insert_with(|| Vec::new());
            comments_for_this_kind.push(Comment {
                contents: contents.to_string(),
                file: path.display().to_string(),
                location: format!("line {}", wher.line),
            });
        }
    }
}

struct TodoVisitor<'a> {
    path: &'a Path,
    tracker: &'a mut CommentTracker,
}

impl<'a> TodoVisitor<'a> {
    fn new(path: &'a Path, tracker: &'a mut CommentTracker) -> TodoVisitor<'a> {
        TodoVisitor { path, tracker }
    }
}

impl<'a, 'ast: 'a> syn::visit::Visit<'ast> for TodoVisitor<'a> {
    fn visit_attribute(&mut self, i: &'ast syn::Attribute) {
        syn::visit::visit_attribute(self, i);

        let contents = i.tokens.to_token_stream().to_string();
        self.tracker.found_possible_comment(
            &contents,
            self.path,
            i.span().start(),
        );
    }
}

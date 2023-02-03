use anyhow::ensure;
use anyhow::Context;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::Path;

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
        println!("comments with \"{}\": {}", label, comments.len());
        for c in comments {
            println!(
                "  found {:?} in file {} line {}",
                label, c.file, c.location
            );
            println!(
                "{}",
                c.contents
                    .lines()
                    .map(|l| format!("    {}\n", l))
                    .collect::<Vec<_>>()
                    .join("")
            );
        }
    }

    println!("SUMMARY:\n");
    for (label, comments) in &tracker.comments_by_kind {
        println!("comments with \"{}\": {}", label, comments.len());
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
    let contents = std::io::read_to_string(&file)
        .with_context(|| format!("read {:?}", path.display()))?;
    let chunker = FileChunker::new(&contents);
    for (line, chunk) in chunker {
        tracker.found_possible_comment(&chunk, path, line);
    }

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
        line: usize,
    ) {
        let mut found_kinds = BTreeSet::new();

        for line in contents.lines() {
            let words = line.split_whitespace();
            for word in words {
                if word.starts_with("XXX")
                    || word.starts_with("FIXME")
                    || word.starts_with("TODO")
                {
                    let mut label = word;
                    if word.ends_with(':') || word.ends_with('-') {
                        label = &word[0..word.len() - 1];
                    }
                    found_kinds.insert(label);
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
                location: format!("line {}", line),
            });
        }
    }
}

/// "Parses" a file in a very limited sense by dividing it into chunks of
/// comments
// It's tempting to use the "syn" crate for this, but it's not that easy to
// visit all of the non-doc comments in a file.
struct FileChunker<'a> {
    lines: std::iter::Enumerate<std::str::Lines<'a>>,
}

impl<'a> FileChunker<'a> {
    fn new(input: &'a str) -> FileChunker {
        FileChunker { lines: input.lines().enumerate() }
    }

    fn join(lines: &[&str]) -> String {
        lines.iter().map(|l| format!("{}\n", l)).collect::<Vec<_>>().join("")
    }
}

enum FileState {
    NoComment,
    InLineComment(usize),
    InBlockComment(usize),
}

impl<'a> Iterator for FileChunker<'a> {
    type Item = (usize, String);

    fn next(&mut self) -> Option<Self::Item> {
        // precondition: we are not currently in a comment.
        let mut state = FileState::NoComment;
        let mut lines = Vec::new();
        while let Some((line_numz, raw_line)) = self.lines.next() {
            let line = raw_line.trim_start().trim_end();

            match state {
                FileState::NoComment => {
                    if line.starts_with("//") {
                        // This won't handle comments on the same line as source
                        // code.  We don't do this often.
                        lines.push(line);
                        state = FileState::InLineComment(line_numz + 1);
                    } else if line.starts_with("/*") && !line.contains("*/") {
                        // This won't handle nested comments.  We don't do this
                        // often.
                        lines.push(line);
                        state = FileState::InBlockComment(line_numz + 1);
                    }

                    // Keep reading while we haven't found a comment.
                }

                FileState::InLineComment(start) => {
                    if line.starts_with("//") {
                        // We're still in a line comment.  Keep reading.
                        lines.push(line);
                    } else {
                        // We got to the end of a line comment.  Emit it.
                        return Some((start, Self::join(&lines)));
                    }
                }

                FileState::InBlockComment(start) => {
                    lines.push(line);
                    if line == "*/" {
                        // We got to the end of the block comment.  Emit it.
                        return Some((start, Self::join(&lines)));
                    }
                }
            }
        }

        match state {
            FileState::NoComment => {
                assert_eq!(lines.len(), 0);
                None
            }

            FileState::InLineComment(start) => {
                // TODO include filename
                eprintln!("warning: file ended with a line comment");
                Some((start, Self::join(&lines)))
            }

            FileState::InBlockComment(start) => {
                // TODO include filename
                eprintln!("error: file ended with a line comment");
                Some((start, Self::join(&lines)))
            }
        }
    }
}

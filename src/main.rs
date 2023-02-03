// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Simplistic command-line tool to summarize TODO-like comments

use anyhow::ensure;
use anyhow::Context;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::Path;

fn main() -> Result<(), anyhow::Error> {
    let argv = std::env::args_os().collect::<Vec<_>>();
    ensure!(argv.len() == 2, "usage: todos path/to/file/tree");
    if argv[1] == "-h" || argv[1] == "--help" || argv[1] == "?" {
        eprintln!("usage: todos path/to/file/tree");
        eprintln!("Scans Rust files in the given tree for TODO-like comments");
        eprintln!("and then prints all such comments, grouped by the TODO-");
        eprintln!("like label (e.g., TODO-security)");
        return Ok(());
    }

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

    // Iterate all the found items, invoking do_file() on each one.
    // Since we want to handle all errors the same way, it's easiest to pass the
    // Result directly to do_file() and let it return it or some other error.
    for maybe_entry in walker {
        if let Err(error) = do_file(&mut tracker, maybe_entry) {
            eprintln!("warn: {:#}", error);
        }
    }

    // Print all the comments that we found, grouped by "kind".
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

    // Print a summary of all comments found.
    let mut total = 0;
    println!("SUMMARY:\n");
    for (label, comments) in &tracker.comments_by_kind {
        println!("comments with \"{}\": {}", label, comments.len());
        total += comments.len();
    }

    println!("total comments found: {}", total);

    Ok(())
}

/// Process one file, finding all TODO-like comments
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

    // Read the file.
    println!("reading {:?}", path.display());
    let contents = std::io::read_to_string(&file)
        .with_context(|| format!("read {:?}", path.display()))?;

    // Pull the TODO-like comments out of the file and track them.
    let chunker = CommentIterator::new(&contents);
    for (line, chunk) in chunker {
        tracker.found_possible_comment(&chunk, path, line);
    }

    Ok(())
}

/// Represents a particular comment found in a particular file
struct Comment {
    contents: String,
    file: String,
    location: String,
}

/// Tracks all TODO-like comments found in our search, grouped by a "kind"
///
/// The kind is basically whatever whitespace-separated word we identified as
/// TODO-like.  This might be "TODO" or "XXX" or "TODO-security" or whatever.
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

        // Figure out what "kinds" of comment this is.  There may be any number
        // of these.  A comment might have no TODO-like things in it (in which
        // case we won't track it) or one of them, or more than one (e.g.,
        // "TODO-security" and "TODO-coverage").  We will track the entire
        // comment once for each "kind" that we find in it.
        for word in contents.split_whitespace() {
            if word.starts_with("XXX")
                || word.starts_with("FIXME")
                || word.starts_with("TODO")
            {
                let mut label = word;
                // People use "TODO" and "TODO:" interchangeably.  Treat them
                // the same.
                if word.ends_with(':') {
                    label = &word[0..word.len() - 1];
                }
                found_kinds.insert(label);
            }
        }

        for k in found_kinds {
            let comments_for_this_kind = self
                .comments_by_kind
                .entry(k.to_string())
                .or_insert_with(Vec::new);
            comments_for_this_kind.push(Comment {
                contents: contents.to_string(),
                file: path.display().to_string(),
                location: format!("line {}", line),
            });
        }
    }
}

/// "Parses" a file (in a very limited sense), emitting the comments found in it
// It's tempting to use the "syn" crate for this, but it's not that easy to
// visit all of the non-doc comments in a file.
struct CommentIterator<'a> {
    lines: std::iter::Enumerate<std::str::Lines<'a>>,
}

impl<'a> CommentIterator<'a> {
    pub fn new(input: &'a str) -> CommentIterator {
        CommentIterator { lines: input.lines().enumerate() }
    }

    fn join(lines: &[&str]) -> String {
        lines.iter().map(|l| format!("{}\n", l)).collect::<Vec<_>>().join("")
    }
}

impl<'a> Iterator for CommentIterator<'a> {
    type Item = (usize, String);

    fn next(&mut self) -> Option<Self::Item> {
        /// parser state
        enum FileState {
            /// not currently inside a comment
            NoComment,
            /// currently inside a line comment
            InLineComment(usize),
            /// currently inside a block comment
            InBlockComment(usize),
        }

        // Precondition: we are not currently in a comment.
        let mut state = FileState::NoComment;

        // Keep track of the lines in the current comment.
        let mut lines = Vec::new();

        // Read lines until we run out of lines in the file or return early.
        while let Some((line_numz, raw_line)) = self.lines.next() {
            let line = raw_line.trim_start().trim_end();

            match state {
                FileState::NoComment => {
                    if line.starts_with("//") {
                        // We've found the start of a line comment.
                        //
                        // TODO This won't handle comments on the same line as
                        // source code.  We don't do this often.
                        lines.push(line);
                        state = FileState::InLineComment(line_numz + 1);
                    } else if line.starts_with("/*") && !line.contains("*/") {
                        // We've found the start of a block comment.
                        //
                        // TODO This won't handle nested comments.  We don't do
                        // this often.
                        lines.push(line);
                        state = FileState::InBlockComment(line_numz + 1);
                    }

                    // We haven't found a comment yet.  Skip this line and
                    // continue the loop.
                }

                FileState::InLineComment(start) => {
                    if !line.starts_with("//") {
                        // We got to the end of a line comment.  Emit it.
                        return Some((start, Self::join(&lines)));
                    } else {
                        // We're still in a line comment.  Keep reading.
                        lines.push(line);
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
                // We got to the end of the file without finding any more
                // comments.  We ought not to have accumulated any lines.
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

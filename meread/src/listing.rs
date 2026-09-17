//! Browsable directory listings.
//!
//! A listing is generated *markdown*, not a bespoke html page, so it flows through the same
//! rendering pipeline as a real document and gets the github styling, the in-memory index cache
//! and live reload for free.

use std::{fmt::Write, fs, io, path::Path};

use crate::{is_markdown, url::encode_path_segment};

/// Names never shown in a listing. `.git` is enormous and never interesting; other dotfiles are
/// listed, since browsing a repo's `.github/` is useful.
const SKIPPED_NAMES: [&str; 1] = [".git"];

/// An entry as it will appear in the listing.
struct Entry {
    /// What to show. Directories keep a trailing `/`.
    label: String,
    /// Where it links, relative to the server root.
    href: String,
    is_dir: bool,
}

/// Generate the markdown for a browsable listing of `dir`.
///
/// `rel_path` is `dir` relative to the server root, with `/` separators and no leading or trailing
/// slash; the empty string means `dir` *is* the root.
pub fn listing_markdown(dir: &Path, rel_path: &str) -> io::Result<String> {
    // things that render as a page of their own, versus things that open as raw bytes
    let (mut pages, mut files): (Vec<Entry>, Vec<Entry>) = (Vec::new(), Vec::new());

    for dir_entry in fs::read_dir(dir)? {
        let dir_entry = dir_entry?;
        let name = dir_entry.file_name().to_string_lossy().into_owned();

        if SKIPPED_NAMES.contains(&name.as_str()) {
            continue;
        }

        let is_dir = dir_entry.file_type().is_ok_and(|kind| kind.is_dir());
        let is_page = is_dir || is_markdown(Path::new(&name));

        // linking directories with a trailing slash keeps relative links inside the page they
        // resolve to (`img.png` next to `docs/README.md`) pointing at the right directory
        let entry = Entry {
            href: format!(
                "/{}{}{}",
                if rel_path.is_empty() {
                    String::new()
                } else {
                    format!("{}/", rel_path)
                },
                encode_path_segment(&name),
                if is_dir { "/" } else { "" }
            ),
            label: if is_dir { format!("{}/", name) } else { name },
            is_dir,
        };

        if is_page {
            pages.push(entry);
        } else {
            files.push(entry);
        }
    }

    // directories first, then files, each alphabetically and ignoring case
    let sort_key = |entry: &Entry| (!entry.is_dir, entry.label.to_lowercase());
    pages.sort_by_key(sort_key);
    files.sort_by_key(sort_key);

    let title = if rel_path.is_empty() {
        dir.file_name().map_or_else(
            || "/".to_string(),
            |name| name.to_string_lossy().into_owned(),
        )
    } else {
        rel_path.to_string()
    };

    let mut markdown = format!("# {}/\n", title);

    if pages.is_empty() && files.is_empty() {
        markdown.push_str("\n*empty directory*\n");
        return Ok(markdown);
    }

    // raw html rather than markdown links, so that `[`, `_` and `*` in file names need no escaping
    for (heading, entries) in [("Pages", &pages), ("Files", &files)] {
        if entries.is_empty() {
            continue;
        }

        write!(markdown, "\n## {}\n\n<ul>\n", heading).unwrap();
        for entry in entries.iter() {
            writeln!(
                markdown,
                "<li><a href=\"{}\">{}</a></li>",
                escape_html(&entry.href),
                escape_html(&entry.label)
            )
            .unwrap();
        }
        markdown.push_str("</ul>\n");
    }

    Ok(markdown)
}

/// Escape text for inclusion in html body text or a double-quoted attribute.
pub(crate) fn escape_html(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    #[test]
    fn groups_directories_and_markdown_as_pages() {
        let dir = TempDir::new("listing_groups");
        dir.dir("archive");
        dir.dir("drafts");
        dir.file("ideas.md", "");
        dir.file("notes.markdown", "");
        dir.file("diagram.png", "");
        dir.file("scratch.txt", "");

        let markdown = listing_markdown(dir.path(), "").unwrap();
        let pages = markdown.find("## Pages").unwrap();
        let files = markdown.find("## Files").unwrap();

        for page in ["archive/", "drafts/", "ideas.md", "notes.markdown"] {
            let at = markdown.find(&format!(">{}<", page)).unwrap();
            assert!(at > pages && at < files, "{} not listed under Pages", page);
        }
        for file in ["diagram.png", "scratch.txt"] {
            let at = markdown.find(&format!(">{}<", file)).unwrap();
            assert!(at > files, "{} not listed under Files", file);
        }
    }

    #[test]
    fn sorts_directories_first_then_case_insensitively() {
        let dir = TempDir::new("listing_sorts");
        dir.file("beta.md", "");
        dir.file("Alpha.md", "");
        dir.dir("zebra");

        let markdown = listing_markdown(dir.path(), "").unwrap();
        let order: Vec<_> = ["zebra/", "Alpha.md", "beta.md"]
            .iter()
            .map(|name| markdown.find(&format!(">{}<", name)).unwrap())
            .collect();

        assert!(
            order.windows(2).all(|pair| pair[0] < pair[1]),
            "{}",
            markdown
        );
    }

    #[test]
    fn hrefs_are_root_absolute_and_encoded() {
        let dir = TempDir::new("listing_hrefs");
        dir.file("My Notes.md", "");
        dir.dir("sub");

        let markdown = listing_markdown(dir.path(), "docs/deep").unwrap();

        assert!(
            markdown.contains("href=\"/docs/deep/My%20Notes.md\""),
            "{}",
            markdown
        );
        // directories keep their trailing slash, so relative links inside them resolve correctly
        assert!(
            markdown.contains("href=\"/docs/deep/sub/\""),
            "{}",
            markdown
        );
        assert!(markdown.starts_with("# docs/deep/\n"), "{}", markdown);
    }

    #[test]
    fn skips_git_but_keeps_other_dotfiles() {
        let dir = TempDir::new("listing_dotfiles");
        dir.dir(".git");
        dir.dir(".github");
        dir.file(".hidden.md", "");

        let markdown = listing_markdown(dir.path(), "").unwrap();

        assert!(!markdown.contains(".git/"), "{}", markdown);
        assert!(markdown.contains(">.github/<"), "{}", markdown);
        assert!(markdown.contains(">.hidden.md<"), "{}", markdown);
    }

    #[test]
    fn reports_empty_directory() {
        let dir = TempDir::new("listing_empty");

        let markdown = listing_markdown(dir.path(), "").unwrap();

        assert!(markdown.contains("*empty directory*"), "{}", markdown);
        assert!(!markdown.contains("<ul>"), "{}", markdown);
    }

    #[test]
    fn escapes_html_in_names() {
        assert_eq!(escape_html("a<b>&\"c\""), "a&lt;b&gt;&amp;&quot;c&quot;");
    }
}

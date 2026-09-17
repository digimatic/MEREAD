use std::{
    fs::File,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc::Receiver},
    thread,
    time::Duration,
};

use bus::{Bus, BusReader};
use rouille::{Response, ResponseBody, try_or_400, websocket};

use crate::{
    assets::EmbeddedAsset,
    comrak_config::ComrakConfig,
    listing::{escape_html, listing_markdown},
    render::{
        PageMeta, RawMarkdown, RenderedMarkdown, render_markdown_file, render_markdown_to_html,
    },
    url::{decode_percent, encode_path_segment},
};
pub mod assets;
pub mod comrak_config;
pub mod export;
pub mod listing;
pub mod render;
mod url;

#[cfg(test)]
mod test_support;

/// URL prefix (and export subdirectory) reserved for meread's own embedded assets, so that the
/// rest of the URL space can be served from the document root.
pub const ASSET_PREFIX: &str = "~~~meread";

/// `ASSET_PREFIX` as it appears in a request path. Keep in sync with `ASSET_PREFIX` and with the
/// asset URLs in `templates/template.html` and `assets/styles.css`.
const ASSET_URL_PREFIX: &str = "/~~~meread/";

/// File names looked for when a path resolves to a directory, before falling back to a listing.
pub const DIRECTORY_INDEX_NAMES: [&str; 2] = ["README.md", "index.md"];

pub fn serve_and_rebuild_on_receive(
    markdown_content_receiver: Receiver<RawMarkdown>,
    light_mode: bool,
    comrak_config: ComrakConfig,
    address: &str,
    open: bool,
    root_dir: PathBuf,
) -> color_eyre::Result<()> {
    let comrak_config = Arc::new(comrak_config);

    // `resolve_within` compares canonical paths against the root, so a root reached through a
    // symlink (as `meread-nvim` may hand us) would otherwise never match anything
    let root_dir = root_dir.canonicalize().unwrap_or(root_dir);

    let initial_markdown = markdown_content_receiver.recv().unwrap();
    // the path, relative to `root_dir`, that serves the cached document
    let index_path = initial_markdown.file_name.clone();
    let index_breadcrumb = breadcrumb_html(&root_dir, &index_path);
    let rendered_markdown = Arc::new(Mutex::new(RenderedMarkdown::new(
        RawMarkdown {
            file_name: page_title(&root_dir, &index_path),
            ..initial_markdown
        },
        index_breadcrumb,
        light_mode,
        Arc::clone(&comrak_config),
    )?));

    let reload_bus = Arc::new(Mutex::new(Bus::new(1)));
    std::thread::spawn({
        let reload_bus = Arc::clone(&reload_bus);
        let rendered_markdown = Arc::clone(&rendered_markdown);
        move || {
            for RawMarkdown { mut content, .. } in &markdown_content_receiver {
                // debounce
                while let Ok(RawMarkdown { content: newer, .. }) =
                    markdown_content_receiver.recv_timeout(Duration::from_millis(50))
                {
                    content = newer;
                }

                rendered_markdown.lock().unwrap().rebuild(&content).unwrap();
                reload_bus.lock().unwrap().broadcast(());
            }
        }
    });

    if open {
        // the document's own url, not `/`, which is the root of the tree and may be a different
        // page entirely when --root was widened
        open::that(format!("http://{}/{}", address, index_url(&index_path))).ok();
    }

    #[cfg(feature = "stdout")]
    println!(
        "serving {} on http://{}",
        rendered_markdown.lock().unwrap().file_name,
        address
    );

    rouille::start_server(address, {
        move |request| {
            if request.method() != "GET" {
                return Response {
                    status_code: 405, // method not allowed
                    headers: Default::default(),
                    data: ResponseBody::empty(),
                    upgrade: None,
                };
            }

            let url = request.url();

            if url == "/~~~meread-reload" {
                let (response, websocket) =
                    try_or_400!(websocket::start(request, None as Option<&str>));

                let rendered_markdown = Arc::clone(&rendered_markdown);

                let reload_rx = reload_bus.lock().unwrap().add_rx();

                thread::spawn(move || {
                    let ws = websocket.recv().unwrap();
                    reload_handler_thread(ws, rendered_markdown, reload_rx)
                });

                return response;
            }

            if let Some(asset_path) = url.strip_prefix(ASSET_URL_PREFIX) {
                return EmbeddedAsset::create_response(asset_path)
                    .unwrap_or_else(Response::empty_404);
            }

            // `/docs/` and `/docs` name the same thing, and file names may arrive percent-encoded
            let path = decode_percent(url.strip_prefix('/').unwrap());
            let path = path.trim_end_matches('/');

            match resolve(&root_dir, path, &index_path) {
                Resolved::Index => {
                    let rendered_markdown = rendered_markdown.lock().unwrap();
                    Response::html(rendered_markdown.content.clone())
                }
                Resolved::Markdown(markdown_file_path) => {
                    let relative = relative_url(&root_dir, &markdown_file_path);
                    let title = page_title(&root_dir, &relative);
                    let breadcrumb = breadcrumb_html(&root_dir, &relative);
                    let meta = PageMeta {
                        title: &title,
                        breadcrumb: &breadcrumb,
                        light: light_mode,
                        is_index: false,
                    };

                    match render_markdown_file(&markdown_file_path, &meta, &comrak_config) {
                        Ok(html) => Response::html(html),
                        Err(err) => error_response(&relative, &err),
                    }
                }
                Resolved::Directory(dir) => {
                    let relative = relative_url(&root_dir, &dir);
                    let title = page_title(&root_dir, &relative);
                    let breadcrumb = breadcrumb_html(&root_dir, &relative);
                    let meta = PageMeta {
                        title: &title,
                        breadcrumb: &breadcrumb,
                        light: light_mode,
                        is_index: false,
                    };

                    match render_listing(&dir, &relative, &meta, &comrak_config) {
                        Ok(html) => Response::html(html),
                        Err(err) => error_response(&relative, &err),
                    }
                }
                // images, css, .. served straight from disk, from the path `resolve` already
                // checked to be inside the root
                Resolved::File(path) => File::open(&path).map_or_else(
                    |_| Response::empty_404(),
                    |file| {
                        Response::from_file(
                            mime_guess::from_path(&path)
                                .first_or_octet_stream()
                                .to_string(),
                            file,
                        )
                    },
                ),
                Resolved::NotFound => Response::empty_404(),
            }
        }
    })
}

fn render_listing(
    dir: &Path,
    relative: &str,
    meta: &PageMeta,
    comrak_config: &ComrakConfig,
) -> color_eyre::Result<String> {
    let markdown = listing_markdown(dir, relative)?;

    let mut html = String::new();
    render_markdown_to_html(&markdown, meta, comrak_config, &mut html)?;

    Ok(html)
}

fn error_response(what: &str, err: &color_eyre::Report) -> Response {
    Response::text(format!("failed to render {}: {:#}", what, err)).with_status_code(500)
}

enum Resolved {
    /// The document meread was started with, which is kept rendered in memory.
    Index,
    /// Some other markdown file below the root, to be rendered on the fly.
    Markdown(PathBuf),
    /// A directory below the root with no index file, to be shown as a browsable listing.
    Directory(PathBuf),
    /// Not markdown; serve the bytes as they are.
    File(PathBuf),
    NotFound,
}

/// Work out what a request path below `root_dir` should be answered with.
///
/// Every path is resolved against the tree first and only then compared with `index_path`, so that
/// the root keeps a url of its own (`/`) even when the served document lives further down.
///
/// `request_path` is expected to be percent-decoded and free of leading and trailing slashes.
fn resolve(root_dir: &Path, request_path: &str, index_path: &str) -> Resolved {
    let resolved = if request_path.is_empty() {
        Some(root_dir.to_path_buf())
    } else {
        // extensionless links (`[guide](docs/guide)`) are how markdown files refer to each other
        // on github, so fall back to `<path>.md` before giving up
        resolve_within(root_dir, request_path)
            .or_else(|| resolve_within(root_dir, &format!("{}.md", request_path)))
    };

    let Some(path) = resolved else {
        // nothing on disk, but the served document may be a buffer that was never written there
        return if request_path == index_path {
            Resolved::Index
        } else {
            Resolved::NotFound
        };
    };

    if path.is_dir() {
        // the directory itself being the served page has to win over its README, or --list could
        // never show anything
        if relative_url(root_dir, &path) == index_path {
            return Resolved::Index;
        }

        return DIRECTORY_INDEX_NAMES
            .iter()
            .map(|name| path.join(name))
            .find(|candidate| candidate.is_file())
            .map_or(Resolved::Directory(path), |index_file| {
                markdown_or_index(root_dir, index_file, index_path)
            });
    }

    if is_markdown(&path) {
        markdown_or_index(root_dir, path, index_path)
    } else {
        Resolved::File(path)
    }
}

/// Markdown that happens to be the served document is answered from memory, so that it stays live.
fn markdown_or_index(root_dir: &Path, path: PathBuf, index_path: &str) -> Resolved {
    if relative_url(root_dir, &path) == index_path {
        Resolved::Index
    } else {
        Resolved::Markdown(path)
    }
}

/// Resolve `request_path` against `root_dir`, returning `None` if it does not exist or escapes
/// the root (`..`, symlinks out of the tree, ..).
fn resolve_within(root_dir: &Path, request_path: &str) -> Option<PathBuf> {
    let mut candidate = root_dir.to_path_buf();
    for component in request_path.split('/') {
        candidate.push(component);
    }

    let candidate = candidate.canonicalize().ok()?;
    candidate.starts_with(root_dir).then_some(candidate)
}

pub(crate) fn is_markdown(path: &Path) -> bool {
    path.extension().is_some_and(|extension| {
        extension.eq_ignore_ascii_case("md") || extension.eq_ignore_ascii_case("markdown")
    })
}

/// `path`'s location below `root_dir` as a url path: `/` separators, no leading or trailing slash,
/// empty when `path` *is* the root.
pub fn relative_url(root_dir: &Path, path: &Path) -> String {
    path.strip_prefix(root_dir)
        .unwrap_or(path)
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// A relative path as it has to be written in a url.
fn index_url(relative: &str) -> String {
    relative
        .split('/')
        .map(encode_path_segment)
        .collect::<Vec<_>>()
        .join("/")
}

/// What to put in `<title>`: the path below the root, or the root's own name for the root itself.
fn page_title(root_dir: &Path, relative: &str) -> String {
    if relative.is_empty() {
        root_dir.file_name().map_or_else(
            || "/".to_string(),
            |name| name.to_string_lossy().into_owned(),
        )
    } else {
        relative.to_string()
    }
}

/// A trail of links back up the tree, e.g. `meread / docs / guide.md`, with every ancestor
/// clickable and the current page plain.
fn breadcrumb_html(root_dir: &Path, relative: &str) -> String {
    let root_label = page_title(root_dir, "");
    let segments: Vec<&str> = relative.split('/').filter(|s| !s.is_empty()).collect();

    if segments.is_empty() {
        return format!("<span>{}</span>", escape_html(&root_label));
    }

    let mut crumbs = vec![format!("<a href=\"/\">{}</a>", escape_html(&root_label))];

    for (i, segment) in segments.iter().enumerate() {
        if i + 1 == segments.len() {
            crumbs.push(format!("<span>{}</span>", escape_html(segment)));
            continue;
        }

        // ancestors are always directories, hence the trailing slash
        let href = segments[..=i]
            .iter()
            .map(|segment| encode_path_segment(segment))
            .collect::<Vec<_>>()
            .join("/");

        crumbs.push(format!(
            "<a href=\"/{}/\">{}</a>",
            escape_html(&href),
            escape_html(segment)
        ));
    }

    crumbs.join(" / ")
}

fn reload_handler_thread(
    mut ws: websocket::Websocket,
    rendered_markdown: Arc<Mutex<RenderedMarkdown>>,
    reload_rx: BusReader<()>,
) {
    for _ in reload_rx.into_iter() {
        let rendered_markdown = rendered_markdown.lock().unwrap();
        ws.send_text(&rendered_markdown.content).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    fn assert_markdown(resolved: &Resolved, expected: &Path) {
        match resolved {
            Resolved::Markdown(path) => assert_eq!(path, expected),
            _ => panic!("expected markdown at {}", expected.display()),
        }
    }

    #[test]
    fn empty_path_and_index_path_hit_the_cached_document() {
        let dir = TempDir::new("resolve_index");
        dir.file("README.md", "");

        let root = dir.canonical();

        assert!(matches!(resolve(&root, "", "README.md"), Resolved::Index));
        assert!(matches!(
            resolve(&root, "README.md", "README.md"),
            Resolved::Index
        ));
    }

    #[test]
    fn directory_without_index_file_becomes_a_listing() {
        let dir = TempDir::new("resolve_listing");
        dir.dir("notes");
        dir.file("notes/ideas.md", "");

        let root = dir.canonical();

        match resolve(&root, "notes", "") {
            Resolved::Directory(path) => assert_eq!(path, root.join("notes")),
            _ => panic!("expected a listing"),
        }
    }

    #[test]
    fn directory_with_index_file_renders_it() {
        let dir = TempDir::new("resolve_dir_index");
        dir.file("docs/index.md", "");

        let root = dir.canonical();

        assert_markdown(&resolve(&root, "docs", ""), &root.join("docs/index.md"));
    }

    #[test]
    fn extensionless_links_fall_back_to_md() {
        let dir = TempDir::new("resolve_extensionless");
        dir.file("docs/guide.md", "");

        let root = dir.canonical();

        assert_markdown(
            &resolve(&root, "docs/guide", ""),
            &root.join("docs/guide.md"),
        );
    }

    #[test]
    fn non_markdown_files_are_served_as_bytes() {
        let dir = TempDir::new("resolve_file");
        dir.file("diagram.png", "");

        let root = dir.canonical();

        match resolve(&root, "diagram.png", "") {
            Resolved::File(path) => assert_eq!(path, root.join("diagram.png")),
            _ => panic!("expected a file"),
        }
    }

    #[test]
    fn paths_escaping_the_root_are_rejected() {
        let dir = TempDir::new("resolve_escape");
        dir.file("inside/README.md", "");
        dir.file("outside.md", "");

        let root = dir.canonical().join("inside");

        assert!(matches!(
            resolve(&root, "../outside.md", ""),
            Resolved::NotFound
        ));
        assert!(matches!(resolve(&root, "nope.md", ""), Resolved::NotFound));
    }

    #[test]
    fn root_url_browses_the_root_when_the_document_lives_deeper() {
        let dir = TempDir::new("resolve_wider_root");
        dir.file("archive/deep.md", "");

        let root = dir.canonical();

        // `meread archive/deep.md --root .`: `/` is the root listing, not the document
        match resolve(&root, "", "archive/deep.md") {
            Resolved::Directory(path) => assert_eq!(path, root),
            _ => panic!("expected the root listing"),
        }
        assert!(matches!(
            resolve(&root, "archive/deep.md", "archive/deep.md"),
            Resolved::Index
        ));
    }

    #[test]
    fn a_browsed_directory_wins_over_its_readme() {
        let dir = TempDir::new("resolve_list_flag");
        dir.file("README.md", "");

        let root = dir.canonical();

        // `meread . --list`: the listing is the served page, the README a page below it
        assert!(matches!(resolve(&root, "", ""), Resolved::Index));
        assert_markdown(&resolve(&root, "README.md", ""), &root.join("README.md"));
    }

    #[test]
    fn a_document_never_written_to_disk_is_still_served() {
        let dir = TempDir::new("resolve_unsaved");
        let root = dir.canonical();

        // the nvim plugin previews the buffer, which may not exist as a file yet
        assert!(matches!(
            resolve(&root, "unsaved.md", "unsaved.md"),
            Resolved::Index
        ));
    }

    #[test]
    fn breadcrumb_links_every_ancestor() {
        let dir = TempDir::new("breadcrumb");
        let root = dir.canonical();
        let root_label = root.file_name().unwrap().to_string_lossy().into_owned();

        assert_eq!(
            breadcrumb_html(&root, ""),
            format!("<span>{}</span>", root_label)
        );
        assert_eq!(
            breadcrumb_html(&root, "docs/My Notes.md"),
            format!(
                "<a href=\"/\">{}</a> / <a href=\"/docs/\">docs</a> / <span>My Notes.md</span>",
                root_label
            )
        );
    }
}

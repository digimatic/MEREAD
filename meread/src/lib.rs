use std::{
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
    render::{RawMarkdown, RenderedMarkdown, render_markdown_file},
};
pub mod assets;
pub mod comrak_config;
pub mod export;
pub mod render;

/// URL prefix (and export subdirectory) reserved for meread's own embedded assets, so that the
/// rest of the URL space can be served from the document root.
pub const ASSET_PREFIX: &str = "~~~meread";

/// `ASSET_PREFIX` as it appears in a request path. Keep in sync with `ASSET_PREFIX` and with the
/// asset URLs in `templates/template.html` and `assets/styles.css`.
const ASSET_URL_PREFIX: &str = "/~~~meread/";

/// File names looked for when a request resolves to a directory.
const DIRECTORY_INDEX_NAMES: [&str; 2] = ["README.md", "index.md"];

pub fn serve_and_rebuild_on_receive(
    markdown_content_receiver: Receiver<RawMarkdown>,
    light_mode: bool,
    comrak_config: ComrakConfig,
    address: &str,
    open: bool,
    root_dir: PathBuf,
) -> color_eyre::Result<()> {
    let comrak_config = Arc::new(comrak_config);

    let initial_markdown = markdown_content_receiver.recv().unwrap();
    // the path, relative to `root_dir`, that serves the cached document
    let index_path = initial_markdown.file_name.clone();
    let rendered_markdown = Arc::new(Mutex::new(RenderedMarkdown::new(
        initial_markdown,
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
        open::that(format!("http://{}", address)).ok();
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

            let path = url.strip_prefix("/").unwrap();

            match resolve(&root_dir, path, &index_path) {
                Resolved::Index => {
                    let rendered_markdown = rendered_markdown.lock().unwrap();
                    Response::html(rendered_markdown.content.clone())
                }
                Resolved::Markdown(markdown_file_path) => {
                    let title = markdown_file_path
                        .strip_prefix(&root_dir)
                        .unwrap_or(&markdown_file_path)
                        .to_string_lossy();

                    match render_markdown_file(
                        &markdown_file_path,
                        &title,
                        light_mode,
                        &comrak_config,
                    ) {
                        Ok(html) => Response::html(html),
                        Err(err) => Response::text(format!("failed to render {}: {:#}", title, err))
                            .with_status_code(500),
                    }
                }
                // not markdown: let rouille serve it from disk (images, css, ..), 404 if missing
                Resolved::NotMarkdown => rouille::match_assets(request, &root_dir),
            }
        }
    })
}

enum Resolved {
    /// The document meread was started with, which is kept rendered in memory.
    Index,
    /// Some other markdown file below the root, to be rendered on the fly.
    Markdown(PathBuf),
    /// Not a markdown file; serve it as a static asset instead.
    NotMarkdown,
}

/// Work out what a request path below `root_dir` should be answered with.
fn resolve(root_dir: &Path, request_path: &str, index_path: &str) -> Resolved {
    if request_path.is_empty() || request_path == index_path {
        return Resolved::Index;
    }

    // extensionless links (`[guide](docs/guide)`) are how markdown files refer to each other on
    // github, so fall back to `<path>.md` before giving up
    let Some(path) = resolve_within(root_dir, request_path)
        .or_else(|| resolve_within(root_dir, &format!("{}.md", request_path)))
    else {
        return Resolved::NotMarkdown;
    };

    if path.is_dir() {
        return DIRECTORY_INDEX_NAMES
            .iter()
            .map(|name| path.join(name))
            .find(|candidate| candidate.is_file())
            .map_or(Resolved::NotMarkdown, Resolved::Markdown);
    }

    if is_markdown(&path) {
        Resolved::Markdown(path)
    } else {
        Resolved::NotMarkdown
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

fn is_markdown(path: &Path) -> bool {
    path.extension().is_some_and(|extension| {
        extension.eq_ignore_ascii_case("md") || extension.eq_ignore_ascii_case("markdown")
    })
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

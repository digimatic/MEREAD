use clap::{CommandFactory, Parser};
use color_eyre::eyre::{Context, ContextCompat, bail, ensure};
use notify::EventKind;
use notify_debouncer_full::{DebounceEventResult, new_debouncer};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::mpsc,
    time::Duration,
};

use meread::{
    DIRECTORY_INDEX_NAMES, comrak_config::ComrakConfig, export::export, listing::listing_markdown,
    relative_url, render::RawMarkdown, serve_and_rebuild_on_receive,
};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Path to markdown file, or directory to browse
    #[arg(default_value = ".")]
    path: PathBuf,

    /// If supplied, will export the markdown file to HTML in the specified directory
    #[arg(long, short)]
    export_dir: Option<PathBuf>,

    /// Whether to overwrite the export directory if it exists
    #[arg(long, short)]
    force: bool,

    /// Directory whose files are servable, so that links can be followed
    /// [default: the markdown file's own directory]
    #[arg(long, short)]
    root: Option<PathBuf>,

    /// Browse the directory even if it contains a README.md or index.md
    #[arg(long)]
    list: bool,

    /// Address to bind the server to
    #[arg(long, short, default_value = "127.0.0.1:3000")]
    address: String,

    /// Whether to open the browser on serve
    #[arg(long, short)]
    open: bool,

    /// Render page in light-mode style
    #[arg(long, short)]
    light_mode: bool,

    /// Print manpage to stdout and exit
    #[arg(long)]
    generate_manpage: bool,
}

/// What meread was pointed at: the page served at the root url, and the thing the watcher
/// regenerates when the tree changes.
#[derive(Clone)]
enum Target {
    /// A markdown file, rendered as it is.
    Document(PathBuf),
    /// A directory, rendered as a browsable listing of what is in it.
    Directory(PathBuf),
}

impl Target {
    fn path(&self) -> &Path {
        match self {
            Self::Document(path) | Self::Directory(path) => path,
        }
    }
}

/// The markdown the target currently amounts to. `relative` is the target's path below the server
/// root, needed to build listing links.
fn read_target(target: &Target, relative: &str) -> color_eyre::Result<String> {
    match target {
        Target::Document(path) => {
            fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))
        }
        Target::Directory(path) => listing_markdown(path, relative)
            .with_context(|| format!("failed to list {}", path.display())),
    }
}

fn main() -> color_eyre::Result<()> {
    color_eyre::config::HookBuilder::default()
        .display_env_section(false)
        .display_location_section(cfg!(debug_assertions))
        .install()?;

    let args = Args::parse();

    if args.generate_manpage {
        let cmd = Args::command();
        clap_mangen::Man::new(cmd).render(&mut std::io::stdout())?;
        return Ok(());
    }

    let path = args
        .path
        .canonicalize()
        .with_context(|| format!("failed to open {}", args.path.display()))?;

    // a directory renders its README.md (or index.md) when it has one, as it always has; without
    // one, or with --list, it is browsable instead
    let target = if path.is_dir() {
        let index_file = if args.list {
            None
        } else {
            DIRECTORY_INDEX_NAMES
                .iter()
                .map(|name| path.join(name))
                .find(|candidate| candidate.is_file())
        };

        index_file.map_or(Target::Directory(path), Target::Document)
    } else {
        Target::Document(path)
    };

    let comrak_config = ComrakConfig::new(args.light_mode)?;

    if let Some(export_dir) = &args.export_dir {
        // a listing exports to an index.html full of links to files that were never exported,
        // which is worse than no export at all
        let Target::Document(markdown_file_path) = &target else {
            bail!(
                "nothing to export: {} is a directory, --export-dir needs a markdown file",
                target.path().display()
            );
        };

        export(
            markdown_file_path,
            export_dir,
            args.force,
            args.light_mode,
            &comrak_config,
        )?;
        return Ok(());
    }

    // everything below the root is servable, so that links out of the document can be followed.
    // the document's own directory is the smallest root that makes sense; pass --root to serve a
    // wider tree, for instance when a document in a subdirectory links back up.
    let root_dir = match &args.root {
        Some(root) => root
            .canonicalize()
            .with_context(|| format!("failed to open root directory {}", root.display()))?,
        None => match &target {
            Target::Document(markdown_file_path) => markdown_file_path
                .parent()
                .context("trying to serve file in root / or something??")?
                .to_path_buf(),
            Target::Directory(dir) => dir.clone(),
        },
    };

    ensure!(
        target.path().starts_with(&root_dir),
        "{} is not inside the root directory {}",
        target.path().display(),
        root_dir.display()
    );

    // the url the document is served at, relative to the root
    let index_path = relative_url(&root_dir, target.path());

    let (markdown_tx, markdown_rx) = mpsc::channel();
    // needed for initial build
    markdown_tx
        .send(RawMarkdown {
            content: read_target(&target, &index_path)?,
            file_name: index_path.clone(),
        })
        .unwrap();

    let mut debouncer = new_debouncer(Duration::from_millis(10), None, {
        let target = target.clone();
        let index_path = index_path.clone();
        let markdown_tx = markdown_tx.clone();
        move |result: DebounceEventResult| {
            let Ok(events) = result else {
                return;
            };

            if !events.iter().any(|e| {
                matches!(
                    e.kind,
                    EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                )
            }) {
                return;
            }

            // a document only cares about its own file, a listing about anything below it
            if let Target::Document(markdown_file_path) = &target
                && !events
                    .iter()
                    .any(|event| event.paths.contains(markdown_file_path))
            {
                return;
            }

            #[cfg(feature = "stdout")]
            println!("[{}] changed, rebuilding..", jiff::Zoned::now().time());

            let Ok(content) = read_target(&target, &index_path) else {
                return;
            };

            // the receiver is gone once the server shuts down, which is not an error
            markdown_tx
                .send(RawMarkdown {
                    content,
                    file_name: index_path.clone(),
                })
                .ok();
        }
    })
    .context("failed to set up file watcher")?;

    debouncer
        .watch(&root_dir, notify::RecursiveMode::Recursive)
        .with_context(|| format!("failed to watch path: {}", root_dir.display()))?;

    serve_and_rebuild_on_receive(
        markdown_rx,
        args.light_mode,
        comrak_config,
        &args.address,
        args.open,
        root_dir,
    )
}

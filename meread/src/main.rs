use clap::{CommandFactory, Parser};
use color_eyre::eyre::{Context, ContextCompat};
use jiff::Zoned;
use notify::EventKind;
use notify_debouncer_full::{DebounceEventResult, new_debouncer};
use std::{fs, path::PathBuf, sync::mpsc, time::Duration};

use meread::{
    comrak_config::ComrakConfig, export::export, render::RawMarkdown, serve_and_rebuild_on_receive,
};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Path to markdown file or directory containing README.md
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

    let markdown_file_path = if args.path.is_dir() {
        args.path.join("README.md")
    } else {
        args.path
    };

    let comrak_config = ComrakConfig::new(args.light_mode)?;

    if let Some(export_dir) = &args.export_dir {
        export(
            &markdown_file_path,
            export_dir,
            args.force,
            args.light_mode,
            &comrak_config,
        )?;
        return Ok(());
    }

    let markdown_file_path = markdown_file_path
        .canonicalize()
        .with_context(|| format!("failed to open {}", markdown_file_path.display()))?;

    // everything below the root is servable, so that links out of the document can be followed.
    // the document's own directory is the smallest root that makes sense; pass --root to serve a
    // wider tree, for instance when a document in a subdirectory links back up.
    let root_dir = match &args.root {
        Some(root) => root
            .canonicalize()
            .with_context(|| format!("failed to open root directory {}", root.display()))?,
        None => markdown_file_path
            .parent()
            .context("trying to serve file in root / or something??")?
            .to_path_buf(),
    };

    // the url the document is served at, relative to the root
    let index_path = markdown_file_path
        .strip_prefix(&root_dir)
        .with_context(|| {
            format!(
                "{} is not inside the root directory {}",
                markdown_file_path.display(),
                root_dir.display()
            )
        })?
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");

    let (markdown_tx, markdown_rx) = mpsc::channel();
    // needed for initial build
    markdown_tx
        .send(RawMarkdown {
            content: fs::read_to_string(&markdown_file_path).unwrap(),
            file_name: index_path,
        })
        .unwrap();

    let mut debouncer = new_debouncer(Duration::from_millis(10), None, {
        let markdown_file_path = markdown_file_path.clone();
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

            if events
                .iter()
                .any(|event| event.paths.contains(&markdown_file_path))
            {
                let now_time = Zoned::now().time();
                #[cfg(feature = "stdout")]
                println!("[{}] file changed, rebuilding..", now_time);
            }
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

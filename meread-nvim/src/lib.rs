use std::{
    path::{Path, PathBuf},
    sync::mpsc,
};

use itertools::Itertools;
use meread::{comrak_config::ComrakConfig, render::RawMarkdown};
use nvim_oxi::{
    Dictionary, Function, Object,
    api::{
        self, Buffer,
        opts::{BufAttachOpts, OnLinesArgs},
        types::*,
    },
    print,
};

#[nvim_oxi::plugin]
fn meread() -> Dictionary {
    Dictionary::from_iter([("setup", Function::from_fn(setup))])
}

fn setup(_: Object) {
    api::create_user_command(
        "MereadPreview",
        start_preview_and_attach_to_buf_changes,
        &Default::default(),
    )
    .unwrap();
}

fn get_contents_of_nvim_buffer(buffer: &Buffer) -> String {
    let last_line_num = buffer.line_count().unwrap();
    let mut lines = buffer.get_lines(0..=last_line_num, false).unwrap();
    lines.join("\n")
}

fn start_preview_and_attach_to_buf_changes(_: CommandArgs) {
    const LIGHT_MODE: bool = false;
    const ADDRESS: &str = "127.0.0.1:3000";
    const OPEN: bool = true;

    let comrak_config = ComrakConfig::new(LIGHT_MODE).unwrap();

    let current_buffer = nvim_oxi::api::get_current_buf();

    // links out of the buffer are resolved against the directory it lives in
    let root_dir = current_buffer
        .get_name()
        .ok()
        .and_then(|name| {
            PathBuf::from(name.to_string())
                .parent()
                .map(Path::to_path_buf)
        })
        .filter(|parent| parent.is_dir())
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));

    // the url the buffer is served at, relative to that directory, which is what the server
    // matches requests against and builds the breadcrumb from
    let file_name = current_buffer.get_name().map_or_else(
        |_| String::new(),
        |name| meread::relative_url(&root_dir, Path::new(&name.to_string())),
    );

    let (markdown_tx, markdown_rx) = mpsc::channel();
    std::thread::spawn(|| {
        meread::serve_and_rebuild_on_receive(
            markdown_rx,
            LIGHT_MODE,
            comrak_config,
            ADDRESS,
            OPEN,
            root_dir,
        )
    });

    // send initial state of buffer, needed!
    markdown_tx
        .send(RawMarkdown {
            content: get_contents_of_nvim_buffer(&current_buffer),
            file_name: file_name.clone(),
        })
        .unwrap();

    let opts = BufAttachOpts::builder()
        .on_lines(move |(_, buffer, _, _, _, _, _, _, _): OnLinesArgs| {
            let _ = markdown_tx.send(RawMarkdown {
                content: get_contents_of_nvim_buffer(&buffer),
                file_name: file_name.clone(),
            });
            false
        })
        .build();

    current_buffer.attach(true, &opts).unwrap();

    print!("[MEREAD] starting preview on http://{}", ADDRESS);
}

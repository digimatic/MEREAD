use color_eyre::eyre::{Context, OptionExt, ensure};
use std::{fs, path::Path};

use crate::{
    ASSET_PREFIX,
    assets::EmbeddedAsset,
    comrak_config::ComrakConfig,
    render::{PageMeta, render_markdown_to_html},
};

pub fn export(
    markdown_file_path: &Path,
    export_dir: &Path,
    force: bool,
    light_mode: bool,
    comrak_config: &ComrakConfig,
) -> color_eyre::Result<()> {
    ensure!(
        force || !export_dir.exists(),
        "export directory {:?} already exists and --force was not supplied",
        export_dir
    );

    fs::create_dir_all(export_dir).context("failed to create export directory")?;

    let markdown_content = fs::read_to_string(markdown_file_path)
        .context("failed to read markdown file for export")?;

    let markdown_file_name = markdown_file_path.file_name().unwrap();

    let mut rendered_html = String::new();
    render_markdown_to_html(
        &markdown_content,
        // a single exported page has no tree to navigate, so no breadcrumb
        &PageMeta {
            title: markdown_file_name.to_str().unwrap(),
            breadcrumb: "",
            light: light_mode,
            is_index: true,
        },
        comrak_config,
        &mut rendered_html,
    )?;

    fs::write(export_dir.join("index.html"), rendered_html).context("failed to write HTML")?;

    // assets live under their own prefix, matching the urls in the rendered html
    let asset_dir = export_dir.join(ASSET_PREFIX);
    fs::create_dir_all(&asset_dir).context("failed to create asset directory")?;

    for path in EmbeddedAsset::iter() {
        fs::write(
            asset_dir.join(path.as_ref()),
            EmbeddedAsset::get(&path)
                .ok_or_eyre("failed getting asset")?
                .data,
        )?;
    }

    #[cfg(feature = "stdout")]
    println!("Exported to {}", export_dir.display());

    Ok(())
}

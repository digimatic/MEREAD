<a href="https://github.com/sermuns/meread">
  <img alt="MEREAD" src="media/banner.png">
</a>

<div align="center">
  <p>
  <em>
    Preview GitHub flavored markdown offline
  </em>
  </p>
  <a href="https://github.com/sermuns/meread/releases/latest">
    <img alt="release-badge" src="https://img.shields.io/github/v/release/sermuns/meread.svg"></a>
  <a href="https://github.com/sermuns/meread/blob/main/LICENSE">
    <img alt="WTFPL" src="https://img.shields.io/badge/License-WTFPL-brightgreen.svg"></a>
</div>

---

MEREAD is a command-line tool for previewing Markdown files as they will be presented on GitHub, all completely locally and offline.

## Motivation

I was surprised to find no _simple_ tool that would allow me to preview Markdown files as they would be rendered on GitHub.

I wanted a tool that is:

- shipped as a single binary
- fast
- accurate to GitHub's rendering
- offline

There are other tools that get the job done, better or worse, but they all have some drawbacks that I wanted to avoid:

| Tool                                                                     | Written in | Biggest drawback                                                                                |
| ------------------------------------------------------------------------ | ---------- | ----------------------------------------------------------------------------------------------- |
| [grip](https://github.com/joeyespo/grip)                                 | Python     | Uses GitHub's markdown API to render Markdown files, causing unnecessary usage of web requests. |
| [gh markdown-preview](https://github.com/yusukebe/gh-markdown-preview)   | Go         | Is meant to be used as extension in `gh`, GitHub's CLI.                                         |
| [markdown-preview.nvim](https://github.com/iamcco/markdown-preview.nvim) | Typescript | Requires Neovim.                                                                                |

## The Neovim plugin, `meread-nvim`

https://github.com/user-attachments/assets/63021cf3-2843-4276-b126-9b99dd745de3

### Installation

For example, using [`vim.pack`](https://neovim.io/doc/user/pack/#vim.pack):

```lua
vim.pack.add {
	{
		src = 'https://github.com/sermuns/MEREAD',
		version = vim.version.range('1'),
	},
}
```

Then, you have to run the `setup` to get the functions loaded. It is also useful to bind the preview action to some keybind.

Advisably, you would do this in a `ftplugin` for markdown by creating the file under `~/.config/nvim/ftplugin/markdown.lua`:

```lua
require('meread').setup {}

-- start preview by pressing F10
vim.keymap.set(
	'n',
	'<F10>',
	function()
		vim.cmd "MereadPreview"
	end
)
```

## The command-line tool, `meread`

https://github.com/user-attachments/assets/cae9935d-fba3-47aa-9ff7-bab59a070802

```present cargo run -- -h
preview github flavored markdown locally

Usage: meread [OPTIONS] [PATH]

Arguments:
  [PATH]  Path to markdown file, or directory to browse [default: .]

Options:
  -e, --export-dir <EXPORT_DIR>  If supplied, will export the markdown file to HTML in the specified directory
  -f, --force                    Whether to overwrite the export directory if it exists
  -r, --root <ROOT>              Directory whose files are servable, so that links can be followed [default: the markdown file's own directory]
      --list                     Browse the directory even if it contains a README.md or index.md
  -a, --address <ADDRESS>        Address to bind the server to [default: 127.0.0.1:3000]
  -o, --open                     Whether to open the browser on serve
  -l, --light-mode               Render page in light-mode style
      --generate-manpage         Print manpage to stdout and exit
  -h, --help                     Print help
  -V, --version                  Print version
```

### Browsing

Point MEREAD at a directory and it renders that directory's `README.md` (or `index.md`), as it
always has. A directory without either is shown as a browsable listing instead, and `--list` gives
you that listing even when there is a `README.md`:

```bash
meread notes/          # listing, if notes/ has no README.md
meread . --list        # listing, even though this repo has one
```

Everything below the served directory is browsable: links between markdown files are followed and
rendered on the fly, images and other files are served as they are, and every page carries a
breadcrumb trail back up the tree. Use `--root` to widen what is servable, for instance when a
document in a subdirectory links back up.

### Installation

#### From prebuilt binaries

For each version, prebuilt binaries are automatically built for Linux, MacOS and Windows.

- You can download and unpack the
  latest release from the [releases page](https://github.com/sermuns/meread/releases/latest).

- Using [`cargo-binstall`](https://github.com/cargo-bins/cargo-binstall):

  ```bash
  cargo binstall meread
  ```

- Using [`ubi`](https://github.com/houseabsolute/ubi):

  ```bash
  ubi -p sermuns/meread
  ```

- Using [`mise`](https://github.com/jdx/mise):

  ```bash
  # `ubi` under the hood
  mise use -g ubi:sermuns/meread
  ```

  ```bash
  # `cargo-binstall` under the hood
  mise use -g cargo:meread
  ```

- Using nix flakes (one-off):

  ```bash
  nix run github:sermuns/meread
  ```

- Using nix flakes:
  1. Add MEREAD to your system `flake.nix`'s inputs

     ```nix
     {
       inputs = {
         nixpkgs.url = "github:NixOS/nixpkgs/unstable";
         meread = {
           url = "github:sermuns/meread";
           inputs.nixpkgs.follows = "nixpkgs";
         };
       };
     }
     ```

  2. Add the MEREAD package to your `environment.systemPackages` list

     ```nix
     {
       pkgs,
       inputs,
     }: {
       environment.systemPackages = [
         inputs.meread.packages.${pkgs.stdenv.hostPlatform.system}.default
       ];
     }
     ```

  3. Rebuild your system with `nixos-rebuild` (or `darwin-rebuild` on MacOS)

#### From source

- ```bash
  cargo install meread
  ```

- ```bash
  git clone https://github.com/sermuns/meread
  cd meread
  cargo install
  ```

### Manpages

Can be installed by

```bash
mkdir -p ~/.local/share/man/man1/
meread --generate-manpage > ~/.local/share/man/man1/meread.1
```

mod app;
mod doc;
mod formats;
mod layout;
mod marks;
mod spread;
mod view;

use std::io::Read;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;

use formats::Format;

pub const NAME: &str = env!("CARGO_PKG_NAME");

/// Long text, laid out as facing pages you turn.
///
/// Space turns the page, b turns back, m bookmarks, g opens the contents,
/// q closes the book. Inside herdr the pane is split and the book opens as
/// a spread across two panes; elsewhere it shows one page at a time.
#[derive(Parser)]
#[command(name = NAME, version)]
struct Cli {
    /// How to read the input. Never guessed.
    #[arg(long, value_enum, required_unless_present = "follow")]
    format: Option<Format>,

    /// The file to read, or `-` for stdin.
    #[arg(required_unless_present = "follow")]
    file: Option<PathBuf>,

    /// Show one page at a time, even inside herdr.
    #[arg(long)]
    single: bool,

    /// Internal: draw the right-hand page for the reader listening here.
    #[arg(long, hide = true, conflicts_with_all = ["format", "file"])]
    follow: Option<PathBuf>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Some(sock) = cli.follow {
        return spread::follow(&sock);
    }
    let (Some(format), Some(file)) = (cli.format, cli.file) else {
        unreachable!("clap requires both without --follow");
    };

    let (bytes, name, book) = if file.as_os_str() == "-" {
        let mut buf = Vec::new();
        std::io::stdin().read_to_end(&mut buf).context("reading stdin")?;
        (buf, "stdin".to_string(), None)
    } else {
        let bytes = std::fs::read(&file).with_context(|| format!("reading {}", file.display()))?;
        let name = file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let book = std::fs::canonicalize(&file)?.to_string_lossy().into_owned();
        (bytes, name, Some(book))
    };
    let doc = formats::load(format, bytes, &name)?;
    app::run(doc, book, cli.single)
}

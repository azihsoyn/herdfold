mod api;
mod app;
mod attach;
mod cli;
mod doc;
mod formats;
mod herdr;
mod layout;
mod marks;
mod server;
mod view;

use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use formats::Format;

pub const NAME: &str = env!("CARGO_PKG_NAME");

/// Long text, laid out as facing pages you turn.
///
/// Space turns the page, b turns back, m bookmarks, g opens the contents,
/// < and > shorten and lengthen the rows, q or Esc closes the book. Inside
/// herdr the pane is split and the book opens as a spread across two panes;
/// elsewhere it shows one page at a time.
#[derive(Parser)]
#[command(name = NAME, version, subcommand_negates_reqs = true, args_conflicts_with_subcommands = true)]
struct Cli {
    /// How to read the input. Never guessed.
    #[arg(long, value_enum, required = true)]
    format: Option<Format>,

    /// The file to read, or `-` for stdin.
    #[arg(required = true)]
    file: Option<PathBuf>,

    /// Show one page at a time, even inside herdr.
    #[arg(long)]
    no_spread: bool,

    /// Longest row, in columns [default: the one last set with < / >, else 72].
    #[arg(long, value_name = "COLS", value_parser = clap::value_parser!(u16).range(24..=240))]
    measure: Option<u16>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Reader helpers over the socket API
    #[command(subcommand)]
    Reader(ReaderCommand),
    /// Inspect the socket API
    #[command(subcommand)]
    Api(ApiCommand),
}

#[derive(Subcommand)]
enum ReaderCommand {
    /// Draw the right-hand page of the reader at $HERDBOOK_SOCKET_PATH
    Attach,
}

#[derive(Subcommand)]
enum ApiCommand {
    /// Print or write the bundled API schema
    Schema {
        #[arg(long, conflicts_with = "output")]
        json: bool,
        #[arg(long, value_name = "PATH")]
        output: Option<PathBuf>,
    },
}

fn main() -> Result<ExitCode> {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Reader(ReaderCommand::Attach)) => Ok(cli::finish("reader:attach", attach::run())),
        Some(Command::Api(ApiCommand::Schema { json, output })) => {
            Ok(cli::finish("api:schema", cli::api_schema(json, output)))
        }
        None => {
            let (Some(format), Some(file)) = (cli.format, cli.file) else {
                unreachable!("clap requires both without a subcommand");
            };
            open(format, file, cli.no_spread, cli.measure.map(usize::from))?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn open(format: Format, file: PathBuf, no_spread: bool, measure: Option<usize>) -> Result<()> {
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
    app::run(doc, book, !no_spread, measure)
}

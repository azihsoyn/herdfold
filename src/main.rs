mod api;
mod app;
mod attach;
mod cli;
mod client;
mod doc;
mod formats;
mod herdr;
mod keys;
mod layout;
mod marks;
mod note;
mod server;
mod turn;
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
/// < and > shorten and lengthen the rows, a turns the page-turn animation
/// on or off, q or Esc closes the book. Inside herdr the pane is split and
/// the book opens as a spread across two panes; elsewhere it shows one page
/// at a time.
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

    /// Longest row, in columns, for this run [default: as last set for this book with < / >, else as last set for any book, else 72].
    #[arg(long, value_name = "COLS", value_parser = clap::value_parser!(u16).range(24..=240))]
    measure: Option<u16>,

    /// Draw page turns this run [default: as last toggled with `a`, else drawn].
    #[arg(long, overrides_with = "no_animation")]
    animation: bool,

    /// Turn pages at once this run, without drawing the turn.
    #[arg(long, overrides_with = "animation")]
    no_animation: bool,

    /// The agent `?` asks: a herdr agent name or pane id [default: one in this tab, else in this workspace].
    #[arg(long, value_name = "NAME|PANE")]
    agent: Option<String>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Reader helpers over the socket API
    #[command(subcommand)]
    Reader(ReaderCommand),
    /// Notes in the book open in the reader at $HERDBOOK_SOCKET_PATH
    #[command(subcommand)]
    Note(NoteCommand),
    /// Inspect the socket API
    #[command(subcommand)]
    Api(ApiCommand),
    /// Manage local configuration ($XDG_CONFIG_HOME/herdbook/config.toml)
    #[command(subcommand)]
    Config(ConfigCommand),
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// Validate config.toml and print diagnostics
    Check,
    /// Back up config.toml and remove custom keybindings
    ResetKeys,
}

#[derive(Subcommand)]
enum NoteCommand {
    /// Write a note in the book
    Add {
        /// The note.
        text: String,
        /// Source line the note is on [default: the page open now].
        #[arg(long)]
        line: Option<usize>,
        /// Character offset into that line.
        #[arg(long, requires = "line")]
        offset: Option<usize>,
        /// On the page, or on the row holding the place.
        #[arg(long, value_enum, default_value = "page")]
        anchor: marks::Anchor,
        /// Who wrote it.
        #[arg(long, value_enum, default_value = "agent")]
        by: marks::Author,
        /// The question the note answers.
        #[arg(long)]
        question: Option<String>,
    },
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
        Some(Command::Reader(ReaderCommand::Attach)) => {
            Ok(cli::finish("reader:attach", attach::run()))
        }
        Some(Command::Note(NoteCommand::Add {
            text,
            line,
            offset,
            anchor,
            by,
            question,
        })) => {
            let params = note::params(text, note::place(line, offset), anchor, by, question);
            Ok(cli::finish("note:add", note::add(params)))
        }
        Some(Command::Config(ConfigCommand::Check)) => {
            Ok(cli::finish("config:check", keys::check()))
        }
        Some(Command::Config(ConfigCommand::ResetKeys)) => {
            Ok(cli::finish("config:reset-keys", keys::reset()))
        }
        Some(Command::Api(ApiCommand::Schema { json, output })) => {
            Ok(cli::finish("api:schema", cli::api_schema(json, output)))
        }
        None => {
            let (Some(format), Some(file)) = (cli.format, cli.file) else {
                unreachable!("clap requires both without a subcommand");
            };
            open(
                format,
                file,
                cli.no_spread,
                cli.measure.map(usize::from),
                match (cli.animation, cli.no_animation) {
                    (true, _) => Some(true),
                    (_, true) => Some(false),
                    _ => None,
                },
                cli.agent,
            )?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn open(
    format: Format,
    file: PathBuf,
    no_spread: bool,
    measure: Option<usize>,
    animate: Option<bool>,
    agent: Option<String>,
) -> Result<()> {
    let (bytes, name, book) = if file.as_os_str() == "-" {
        let mut buf = Vec::new();
        std::io::stdin()
            .read_to_end(&mut buf)
            .context("reading stdin")?;
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
    app::run(doc, book, !no_spread, measure, animate, agent)
}

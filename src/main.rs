mod api;
mod app;
mod attach;
mod cli;
mod client;
mod doc;
mod export;
mod formats;
mod herdr;
mod highlight;
mod keys;
mod layout;
mod log;
mod marks;
mod note;
mod pictures;
mod server;
mod shelf;
mod turn;
mod view;

use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use api::{Call, EmptyParams, IndexParams};
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
    #[arg(long, value_enum, requires = "file")]
    format: Option<Format>,

    /// The file to read, or `-` for stdin. With neither, the shelf of books
    /// read before.
    #[arg(requires = "format")]
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

    /// Highlight code with this theme this run [default: as named under `[highlight]` in config.toml, else `ansi`]. `herdfold config themes` lists them.
    #[arg(long, value_name = "NAME")]
    theme: Option<String>,

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
    /// Notes: written into the open reader, or exported from a book
    #[command(subcommand)]
    Note(NoteCommand),
    /// The reading log: one JSON Lines file a session
    #[command(subcommand)]
    Log(LogCommand),
    /// The open book's contents
    #[command(subcommand)]
    Contents(ContentsCommand),
    /// Search the open book
    #[command(subcommand)]
    Search(SearchCommand),
    /// Links in the open book: notes and cross-references
    #[command(subcommand)]
    Link(LinkCommand),
    /// Bookmarks in the open book
    #[command(subcommand)]
    Bookmark(BookmarkCommand),
    /// Ask an agent about the open book
    #[command(subcommand)]
    Question(QuestionCommand),
    /// Print the open reader's events as they happen, one JSON line each
    Events {
        /// Event types (e.g. `note.added`, `reader.moved`) [default: all but page.shown].
        types: Vec<String>,
    },
    /// Inspect the socket API
    #[command(subcommand)]
    Api(ApiCommand),
    /// Manage local configuration ($XDG_CONFIG_HOME/herdfold/config.toml)
    #[command(subcommand)]
    Config(ConfigCommand),
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// Validate config.toml and print diagnostics
    Check,
    /// Back up config.toml and remove custom keybindings
    ResetKeys,
    /// List the themes code can be highlighted with
    Themes,
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
    /// The notes in the open book
    List,
    /// Rewrite a note, or recolour a marker
    Update {
        index: usize,
        #[arg(long)]
        text: Option<String>,
        #[arg(long, value_enum)]
        color: Option<marks::Ribbon>,
    },
    Remove {
        index: usize,
    },
    /// Print the notes, markers and bookmarks written in a book, as Markdown
    Export {
        /// How to read the book. Never guessed.
        #[arg(long, value_enum)]
        format: Format,
        /// The book.
        file: PathBuf,
        /// JSON instead, in herdr's reply envelope.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum LogCommand {
    /// List sessions, newest first
    List {
        /// Only sessions with this book.
        #[arg(long, value_name = "FILE")]
        book: Option<PathBuf>,
    },
    /// Show a session: its summary and every record
    Show { session: String },
    /// Write every record as JSON Lines to stdout, oldest first
    Export {
        /// Only sessions with this book.
        #[arg(long, value_name = "FILE")]
        book: Option<PathBuf>,
    },
    /// Read records written by `export` (a file, or `-` for stdin)
    Import { file: PathBuf },
    /// Open the book a session read, where it left off
    Resume { session: String },
}

#[derive(Subcommand)]
enum ReaderCommand {
    /// Draw the right-hand page of the reader at $HERDFOLD_SOCKET_PATH
    Attach,
    /// List the readers open on this machine, with what each has open
    List,
    /// What the reader has open, where, and how it is set
    State,
    /// Turn the page
    Turn {
        #[arg(value_enum, default_value = "forward")]
        direction: api::TurnDirection,
    },
    /// Go to a page, a chapter or a place (a jump `reader back` comes back from)
    GoTo {
        /// Page, from 1.
        #[arg(long, conflicts_with_all = ["chapter", "line"])]
        page: Option<usize>,
        /// Chapter, by its index in `contents list`.
        #[arg(long, conflicts_with = "line")]
        chapter: Option<usize>,
        /// Source line.
        #[arg(long)]
        line: Option<usize>,
        /// Character offset into that line.
        #[arg(long, requires = "line")]
        offset: Option<usize>,
    },
    /// Go back to where the last jump left from
    Back,
    /// Change how the book is set
    Set {
        #[arg(long, value_enum)]
        direction: Option<marks::Direction>,
        #[arg(long, value_enum)]
        writing: Option<marks::Writing>,
        /// Longest row, in columns.
        #[arg(long)]
        measure: Option<usize>,
        /// Draw page turns.
        #[arg(long)]
        animation: Option<bool>,
        #[arg(long, value_enum)]
        note_display: Option<marks::NoteDisplay>,
    },
    /// Close the book
    Close,
}

#[derive(Subcommand)]
enum ContentsCommand {
    /// The chapters, with the pages they open on
    List,
}

#[derive(Subcommand)]
enum SearchCommand {
    /// Find words in the book (nothing moves)
    Run { query: String },
}

#[derive(Subcommand)]
enum LinkCommand {
    /// The links, with where they lead
    List {
        /// Only those on the open pages.
        #[arg(long)]
        open: bool,
    },
    /// Go where a link leads
    Follow { index: usize },
}

#[derive(Subcommand)]
enum BookmarkCommand {
    List,
    /// Bookmark a page [default: the first open page]
    Add {
        #[arg(long)]
        page: Option<usize>,
        #[arg(long, value_enum)]
        color: Option<marks::Ribbon>,
    },
    /// Recolour a bookmark
    Update {
        index: usize,
        #[arg(long, value_enum)]
        color: marks::Ribbon,
    },
    Remove {
        index: usize,
    },
}

#[derive(Subcommand)]
enum QuestionCommand {
    /// Ask about the open pages, or a place (the agent answers in its pane)
    Ask {
        question: String,
        #[arg(long)]
        line: Option<usize>,
        #[arg(long, requires = "line")]
        offset: Option<usize>,
    },
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
    if let Some(theme) = &cli.theme {
        if !highlight::is_theme(theme) {
            let e = cli::CliError::new(
                "unknown_theme",
                format!("no theme {theme:?}; `herdfold config themes` lists them"),
            );
            return Ok(cli::finish("open", Err(e)));
        }
        highlight::use_theme(theme);
    }
    match cli.command {
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
        Some(Command::Reader(r)) => Ok(reader_command(r)),
        Some(Command::Contents(ContentsCommand::List)) => Ok(cli::finish(
            "contents:list",
            cli::call("contents:list", Call::ContentsList(EmptyParams {})),
        )),
        Some(Command::Search(SearchCommand::Run { query })) => Ok(cli::finish(
            "search:run",
            cli::call(
                "search:run",
                Call::SearchRun(api::SearchRunParams { query }),
            ),
        )),
        Some(Command::Link(LinkCommand::List { open })) => Ok(cli::finish(
            "link:list",
            cli::call("link:list", Call::LinkList(api::LinkListParams { open })),
        )),
        Some(Command::Link(LinkCommand::Follow { index })) => Ok(cli::finish(
            "link:follow",
            cli::call("link:follow", Call::LinkFollow(IndexParams { index })),
        )),
        Some(Command::Bookmark(b)) => Ok(bookmark_command(b)),
        Some(Command::Question(QuestionCommand::Ask {
            question,
            line,
            offset,
        })) => {
            let call = Call::QuestionAsk(api::QuestionAskParams {
                question,
                at: note::place(line, offset),
                end: None,
                anchor: if line.is_some() {
                    marks::Anchor::Line
                } else {
                    marks::Anchor::Page
                },
            });
            Ok(cli::finish("question:ask", cli::call("question:ask", call)))
        }
        Some(Command::Events { types }) => Ok(cli::finish("events", cli::events(types))),
        Some(Command::Note(NoteCommand::List)) => Ok(cli::finish(
            "note:list",
            cli::call("note:list", Call::NoteList(EmptyParams {})),
        )),
        Some(Command::Note(NoteCommand::Update { index, text, color })) => Ok(cli::finish(
            "note:update",
            cli::call(
                "note:update",
                Call::NoteUpdate(api::NoteUpdateParams { index, text, color }),
            ),
        )),
        Some(Command::Note(NoteCommand::Remove { index })) => Ok(cli::finish(
            "note:remove",
            cli::call("note:remove", Call::NoteRemove(IndexParams { index })),
        )),
        Some(Command::Note(NoteCommand::Export { format, file, json })) => {
            Ok(cli::finish("note:export", export::run(format, file, json)))
        }
        Some(Command::Config(ConfigCommand::Check)) => {
            Ok(cli::finish("config:check", keys::check()))
        }
        Some(Command::Config(ConfigCommand::Themes)) => {
            let reply = serde_json::json!({
                "id": "cli:config:themes",
                "result": {
                    "type": "themes",
                    "themes": highlight::theme_names(),
                    "current": highlight::configured_theme()
                        .unwrap_or_else(|| highlight::DEFAULT_THEME.to_string()),
                },
            });
            println!("{reply}");
            Ok(ExitCode::SUCCESS)
        }
        Some(Command::Config(ConfigCommand::ResetKeys)) => {
            Ok(cli::finish("config:reset-keys", keys::reset()))
        }
        Some(Command::Log(LogCommand::List { book })) => {
            Ok(cli::finish("log:list", log::list(book)))
        }
        Some(Command::Log(LogCommand::Show { session })) => {
            Ok(cli::finish("log:show", log::show(&session)))
        }
        Some(Command::Log(LogCommand::Export { book })) => {
            Ok(cli::finish("log:export", log::export(book)))
        }
        Some(Command::Log(LogCommand::Import { file })) => {
            Ok(cli::finish("log:import", log::import(file)))
        }
        Some(Command::Log(LogCommand::Resume { session })) => {
            let (book, at) = match log::resume_point(&session) {
                Ok(found) => found,
                Err(e) => return Ok(cli::finish("log:resume", Err(e))),
            };
            let file = PathBuf::from(book.key.unwrap_or_default());
            open(
                book.format,
                file,
                cli.no_spread,
                None,
                None,
                cli.agent,
                Some(at),
            )?;
            Ok(ExitCode::SUCCESS)
        }
        Some(Command::Api(ApiCommand::Schema { json, output })) => {
            Ok(cli::finish("api:schema", cli::api_schema(json, output)))
        }
        None => {
            let (Some(format), Some(file)) = (cli.format, cli.file) else {
                // No book named: the shelf, until it is closed; a book
                // closed comes back to it.
                let mut say = None;
                while let Some((format, file)) = shelf::pick(say.take())? {
                    if let Err(e) = open(
                        format,
                        file,
                        cli.no_spread,
                        None,
                        None,
                        cli.agent.clone(),
                        None,
                    ) {
                        say = Some(format!("{e:#}"));
                    }
                }
                return Ok(ExitCode::SUCCESS);
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
                None,
            )?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn reader_command(r: ReaderCommand) -> ExitCode {
    let (id, call) = match r {
        ReaderCommand::Attach => return cli::finish("reader:attach", attach::run()),
        ReaderCommand::List => return cli::finish("reader:list", cli::readers()),
        ReaderCommand::State => ("reader:state", Call::ReaderState(EmptyParams {})),
        ReaderCommand::Turn { direction } => (
            "reader:turn",
            Call::ReaderTurn(api::ReaderTurnParams { direction }),
        ),
        ReaderCommand::GoTo {
            page,
            chapter,
            line,
            offset,
        } => (
            "reader:go-to",
            Call::ReaderGoTo(api::ReaderGoToParams {
                page,
                chapter,
                at: note::place(line, offset),
            }),
        ),
        ReaderCommand::Back => ("reader:back", Call::ReaderBack(EmptyParams {})),
        ReaderCommand::Set {
            direction,
            writing,
            measure,
            animation,
            note_display,
        } => (
            "reader:set",
            Call::ReaderSet(api::ReaderSetParams {
                direction,
                writing,
                measure,
                animation,
                note_display,
            }),
        ),
        ReaderCommand::Close => ("reader:close", Call::ReaderClose(EmptyParams {})),
    };
    cli::finish(id, cli::call(id, call))
}

fn bookmark_command(b: BookmarkCommand) -> ExitCode {
    let (id, call) = match b {
        BookmarkCommand::List => ("bookmark:list", Call::BookmarkList(EmptyParams {})),
        BookmarkCommand::Add { page, color } => (
            "bookmark:add",
            Call::BookmarkAdd(api::BookmarkAddParams { page, color }),
        ),
        BookmarkCommand::Update { index, color } => (
            "bookmark:update",
            Call::BookmarkUpdate(api::BookmarkUpdateParams { index, color }),
        ),
        BookmarkCommand::Remove { index } => (
            "bookmark:remove",
            Call::BookmarkRemove(IndexParams { index }),
        ),
    };
    cli::finish(id, cli::call(id, call))
}

fn open(
    format: Format,
    file: PathBuf,
    no_spread: bool,
    measure: Option<usize>,
    animate: Option<bool>,
    agent: Option<String>,
    start: Option<layout::Pos>,
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
    let dir = (file.as_os_str() != "-")
        .then(|| file.parent().map(|p| p.to_path_buf()))
        .flatten();
    let doc = formats::load(format, bytes, &name, dir.as_deref())?;
    app::run(
        doc,
        book,
        app::Opening {
            spread: !no_spread,
            measure,
            animate,
            agent,
            format,
            start,
        },
    )
}

//! `herdfold note add`: writes a note into the book open in a reader, the
//! way an agent answering the reader's question keeps its answer.

use crate::api::{Call, NoteAddParams};
use crate::cli::CliError;
use crate::client::Client;
use crate::layout::Pos;
use crate::marks::{Anchor, Author};

/// Sends `note.add` and prints the reply, as herdr's commands print theirs.
pub fn add(params: NoteAddParams) -> Result<(), CliError> {
    let mut client = Client::connect("cli:note:add")?;
    let result = client.call(Call::NoteAdd(params))?;
    let reply = serde_json::json!({ "id": "cli:note:add", "result": result });
    println!("{reply}");
    Ok(())
}

/// The note's place from `--line` / `--offset`, if given.
pub fn place(line: Option<usize>, offset: Option<usize>) -> Option<Pos> {
    line.map(|line| Pos {
        line,
        offset: offset.unwrap_or(0),
    })
}

pub fn params(
    text: String,
    at: Option<Pos>,
    anchor: Anchor,
    by: Author,
    question: Option<String>,
) -> NoteAddParams {
    NoteAddParams {
        text,
        at,
        end: None,
        anchor,
        by,
        question,
    }
}

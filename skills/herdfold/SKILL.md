---
name: herdfold
description: Lay long text (a document, a PR's diff, an EPUB, an Aozora Bunko text, a PDF's text) out as facing pages across two herdr panes for the user to read, turned rather than scrolled. Use when the user wants something long read like a book ("open it in herdfold", "let me read this PR as a book", "open this EPUB", "I want to read <title> from Aozora Bunko"), in any language. Also use to answer a question the reader sent with herdfold's `?` and write the answer back into the book as a note; to act on an open book when asked ("show me this chapter", "open where X comes up", "bookmark this", "read it aloud"); and for the reading log and notes ("what have I read lately?", "open where I left off", "sum up my notes on this book").
---

# herdfold: long things as a book to read

`herdfold` is on PATH (otherwise `brew install azihsoyn/tap/herdfold` or
`cargo install herdfold`). These instructions match the herdfold that
installed them (`herdfold --version`); `herdfold --skill` prints the ones
for the herdfold on PATH. Opened inside herdr, the pane splits in two and
the book opens as a spread. Nothing scrolls: pages are turned.

## Opening

**Always give the format with `--format`** (it is never guessed):
`md` / `epub` / `text` / `diff` / `aozora` (an Aozora Bunko text; Shift_JIS
as it comes) / `pdf-text` (the text taken out of a PDF and set again;
columns, figures and formulas come out broken; a scanned PDF, or one
without a map from its glyphs to text, cannot be read, so say so).

Open it in a pane beside the user:

```sh
ME=$(herdr pane current --current | jq -r .result.pane.pane_id)
PANE=$(herdr pane split "$ME" --direction right --cwd "$PWD" --no-focus | jq -r .result.pane.pane_id)
herdr pane run "$PANE" "herdfold --format md README.md"
```

- A PR: `gh pr diff <number> | herdfold --format diff -` (a chapter a file, additions and deletions coloured, code highlighted)
- A git diff: use `git diff --no-ext-diff` (an external diff tool's output is not a unified diff)
- Aozora Bunko: download the work's "text file (with ruby)" zip, unpack it, and open it with `--format aozora`
- `herdfold` with no arguments opens the shelf of books read before: the answer to "open something I was reading"
- The pane it opens in splits again into the spread; under 100 columns wide it shows one page at a time
- Pictures in Markdown (local files) and EPUBs are shown inside herdr; elsewhere their description is
- When it is open, say which pane it is in and the main keys:
  Space turn / b back / g contents / / search / f links / Backspace back from a jump / V vertical / r read aloud / ? ask / h keys / q close

## Answering the reader's questions

When the reader presses `?`, a prompt reaches this agent through herdr with:

- the book, page and chapter, the question, and the open pages' text (with the lines or sentence chosen, if any)
- at the end, a command to leave a short answer in the book:
  `HERDFOLD_SOCKET_PATH='…' '…/herdfold' note add --line N --offset M --anchor … --question '…' '<your short answer>'`

To answer:

1. **Answer in this pane first** (from the page's text; say so when the book does not say)
2. **Then run the command, with `<your short answer>` replaced by a short answer (one to three sentences)**.
   It goes in as a `✦` note where the question was asked. On success it prints
   `{"id":"cli:note:add","result":{"type":"note_added","page":N}}`
3. If it fails (`reader_not_found` and the like), the book has been closed: the answer in the pane is enough

## Acting on an open book

Everything the reader does is a socket API method, and a CLI command does
the same (the keys call the same API). A command goes to the reader at
`HERDFOLD_SOCKET_PATH`, else to the only reader open. With several open,
pick a `socket` from `herdfold reader list` and run
`HERDFOLD_SOCKET_PATH=<socket> herdfold …`.

- Where the reader is: `herdfold reader state` (book, page, chapter, settings, bookmark and note counts, the voice reading if any)
- Looking without moving the page: `herdfold search run '<words>'` (pages and lines found),
  `herdfold contents list` (chapters and their first pages), `herdfold link list` (notes and cross-references, with the text they lead to)
- Moving, when asked: `herdfold reader go-to --chapter N` / `--page N` / `--line L`,
  `herdfold link follow N`. The reader goes back with `Backspace` (`herdfold reader back`)
- Bookmarks: `herdfold bookmark list` / `add [--page N] [--color C]` / `update N --color C` / `remove N`
- Notes: `herdfold note list` / `add` / `update N --text …` / `remove N`
- Bookmarks and notes are anchored by the text they mark, so they follow it when the book is edited or replaced. One
  not found again shows `"lost": true` in `bookmark list` / `note list`. Do not remove it: ask the reader where it belongs
- Display: `herdfold reader set --writing vertical|horizontal --direction left-to-right|right-to-left --measure N`
- The text as it is read aloud, in sentences with where each is: `herdfold text read [--line L --offset O] [--pages N]`
- Reading aloud, when asked: `herdfold voice start [--engine NAME]` / `voice stop`. `herdfold voice engines` lists the
  voices set up under `[voice]` in config.toml; `herdfold voice say '<text>'` tries one
- What is happening: `herdfold events [note.added reader.moved voice.stopped …]` (one JSON event a line)
- The types: `herdfold api schema --json` (`request` / `success_response` / `event`)

## The reading log and notes

Output is JSON in herdr's shape, `{"id":"cli:…","result":{…}}` (a failure is an `error` on stderr, exit code 1).

- "What have I read lately?": `herdfold log list` (newest first: the book, start and end, from where to where, pages read,
  time, whether it was finished). `--book FILE` for one book
- "Open where I left off": pick a `session` from `log list` and run `herdfold log resume <session>` in a pane made as above
  (it opens where that session ended)
- One session in detail: `herdfold log show <session>`
- "Sum up my notes on this book": `herdfold note export --format <fmt> <FILE>` prints Markdown
  (by chapter: highlighted quotes, notes, questions and answers, bookmarks). `--json` to work on it
- Moving the log: `herdfold log export > log.jsonl` / `herdfold log import log.jsonl`
- The records' types are `log_record` and `note_export` in `herdfold api schema --json`

## Never

- Write to the reader's places, bookmarks, notes and reading log (`~/.local/share/herdfold/`) or settings (`~/.config/herdfold/`) directly. Change them only through the commands (the API)
- Move the page unasked (`reader go-to`, `turn`, `link follow` alike), or start or stop the voice unasked. The pace and the place are the reader's. To look, use `search run` / `reader state` / `link list` / `text read`
- Work the reader by sending keys. Use the commands (the API)
- Close a pane you did not open

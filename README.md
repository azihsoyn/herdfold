# herdfold

Long text, laid out as facing pages you turn.

A fold is where a herder keeps the herd, and what a sheet of paper becomes
when it is folded into two facing pages; here it is two herdr panes, twofold.

![herdfold demo](https://raw.githubusercontent.com/azihsoyn/herdfold/main/demo.gif)

Scrolling has no sense of place; pages do. A page has a fixed amount on it, a
number that says where you are, and a turn that marks where one stretch of
reading ends and the next begins. `less`, `bat` and `ov` treat a long text as
an endless strip; this treats it as a book.

```sh
herdfold --format epub the-book.epub
herdfold --format md README.md
gh pr diff 123 | herdfold --format diff -
```

Inside [herdr](https://herdr.dev) the pane splits in two and the book opens
as a spread: left page in one pane, right page in the other, the text running
on from one to the next. Elsewhere it shows one page at a time.

## Reading

- **Formats are named, never guessed**: `text`, `md` (rendered: headings,
  emphasis, lists, quotes, code blocks, tables), `diff` (each file a
  chapter; stdin works) and `epub`.
- **Pages are cut by the height of the pane**, and rows are held to 72
  columns with margins around them, however wide the pane; `<` and `>`
  change that per book. Chapters open on a new page, set a quarter of the
  way down, as a printed book sets them.
- **A turn is drawn**: the free edge of the sheet (`┃`) crosses the right
  page and then the left. `a` switches that off for good.
- **Books bound on the right read from the right.** `D` turns a book round
  for pages that run right to left, as Japanese books and manga do: the
  right page comes first, turns sweep the other way, and the arrows follow.
  An EPUB that declares `page-progression-direction="rtl"` opens that way.
- **The contents** (`g`) open as a drawer down the side, the chapter you
  are in pointed at; `←` / `→` fold and unfold sections. `Backspace`
  goes back to the page you jumped from, from there or from the list or
  a search.
- **Notes and cross-references** in the book are links: `f` points at
  each on the open pages in turn, or click one; `Enter` shows the passage
  it leads to over the page, and `Enter` again goes there (`Backspace`
  comes back). Markdown footnotes (`[^1]`) and links to headings
  (`[see](#the-heading)`) count, as do an EPUB's links between its pages.
- **Search** (`/` or `Ctrl-F`) finds as you type; `n` / `N` step through
  the finds, `l` lists them with the words around each.
- **Your place is kept** per book, as a position in the text rather than a
  page number, so it survives a change of pane size.

## Marking

- **Bookmarks are ribbons** hanging from the top of a page into the margin,
  in six colours. In a spread, `m` marks the page of the pane you press it
  in. Click a ribbon to recolour it or take it out.
- **Notes** go on a page (`n`, a pencil `✎` by the running head), on a row
  (`v`, a stroke `▎` in the margin), or on text you choose with the mouse.
  `N` shows them as footnotes, in the outer margin, or as marks only.
- **Markers**: drag over text, then `m` lays a highlighter over it. Click a
  marker to recolour it, write its note, or remove it. The book takes the
  mouse for this, since the terminal's own selection would run across
  both panes of a spread; `y` copies the chosen text.
- `l` lists bookmarks and notes together.
- **Take them away**: `herdfold note export --format epub the-book.epub`
  prints what you wrote in a book as Markdown, under its chapters: each
  marker's text quoted, each note, each question and its answer, then the
  bookmarks. `--json` gives the same as data (`note_export` in
  `herdfold api schema --json`).

## Pictures

Inside herdr, pictures in Markdown (local files) and in EPUB books are set
on the page, on rows the layout leaves for them, never split across pages.
PNG, JPEG, GIF, WebP and BMP are read. herdr draws its panes itself, so
the terminal's own image protocols (`imgcat` and the like) do not get
through it; herdfold uses herdr's graphics API instead. Pictures inside an
EPUB are kept in `~/.cache/herdfold/pictures` (`$XDG_CACHE_HOME` if set).
Outside herdr, and for pictures on the web, their description is shown.

## The reading log

Each sitting with a book is a session, kept as a JSON Lines file of its
own in `~/.local/share/herdfold/sessions/` (`$XDG_DATA_HOME` if set): when
the book was opened and closed, the pages shown, bookmarks, notes and
markers, searches and questions, one record a line.

```sh
herdfold log list [--book FILE]     # sessions, newest first, summed up
herdfold log show SESSION           # one session, every record
herdfold log export [--book FILE]   # every record, JSON Lines on stdout
herdfold log import FILE            # records from export; kept ones stay
herdfold log resume SESSION         # the book again, where that session left it
```

```json
{"session":"20261005T120000Z-1a2b3c","time":"2026-10-05T12:00:00Z","type":"page_shown","at":{"line":120,"offset":0,"page":12,"pages":340,"chapter":"Ribbons and notes"}}
```

`herdfold api schema --json` gives the record's schema, under `log_record`.

## Asking the agent

`?` sends a question, with the open pages (and the row or text you chose),
to a herdr agent in the same tab (else the same workspace, or the one named
with `--agent`). It answers in its own pane, and keeps a short answer in the
book as a note (`✦`) by running `herdfold note add`.

## Keys

| Key | |
| --- | --- |
| `Space` / `→` | turn the page (`←` when the book runs right to left) |
| `b` / `←` | turn back |
| `g` | contents |
| `Backspace` / `Ctrl-O` | back to where the last jump (contents, list, search) left from |
| `f` | links here: `Enter` to see where one leads, again to go |
| `/` or `Ctrl-F` | search |
| `m` | bookmark the page (again to remove) |
| `c` | colour of the bookmark here |
| `n` | write a note on the page |
| `v` | choose a row: `Enter` to note it, `?` to ask about it |
| drag | choose text: `m` marker · `n` note · `?` ask · `y` copy |
| click | on a ribbon or a marker: colour, note, remove; on a link: where it leads |
| `l` | bookmarks and notes |
| `N` | notes as footnotes / in the margin / marks only |
| `?` | ask the agent about the open pages |
| `<` / `>` | shorter / longer rows |
| `a` | page-turn animation on / off |
| `D` | pages run left to right / right to left |
| `h` | list the keys |
| `T` | a tip (one greets each book until you tick "don't show again") |
| `q` / `Esc` | close the book |

## Configuration

Every key can be rebound, as herdr's own are, under `[keys]` in
`~/.config/herdfold/config.toml` (`$XDG_CONFIG_HOME` if set). Name an action
and give a key or a list of keys, by herdr's key names; that action loses its
defaults, and `""` leaves it unbound:

```toml
[keys]
next_page = ["l", "space"]
previous_page = ["h", "b"]
help = "?"
ask = "A"
```

The actions: `next_page` `previous_page` `contents` `go_back` `follow_link` `search` `bookmark`
`bookmark_color` `note` `choose_row` `list` `note_display` `ask`
`shorter_rows` `longer_rows` `animation` `direction` `help` `tip` `up` `down`
`enter` `remove` `back` `quit`. `herdfold config check` reports what is wrong
with the file; `herdfold config reset-keys` backs it up and removes `[keys]`.
The key list (`h`) and the tips show the keys as bound.

Your place, bookmarks, notes, row length and direction are kept per book in
`~/.local/share/herdfold/marks.json`; choices that hold for every book (the
animation, how notes are shown, the colours last chosen, tips) in
`settings.json` beside it (`$XDG_DATA_HOME` if set). `--measure COLS`,
`--no-animation` and `--no-spread` decide those for one run.

## What it does not do

- It never reads the text for meaning: pages are cut by height, and
  chapters come only from the input (an EPUB's table of contents, Markdown
  headings, the files of a diff). Plain text has none, and none are guessed.
- It never guesses the format.
- It never scrolls. Only turning.

## Socket API

The reader speaks herdr's protocol: newline-delimited JSON over a Unix
socket, whose path it hands to the panes it opens as `HERDFOLD_SOCKET_PATH`.

```
{"id":"1","method":"ping","params":{}}
{"id":"1","result":{"type":"pong","version":"0.2.0","protocol":1}}

{"id":"2","method":"reader.send_keys","params":{"keys":["space"]}}
{"id":"2","result":{"type":"ok"}}

{"id":"3","method":"events.subscribe","params":{"subscriptions":[{"type":"page.shown"}]}}
{"id":"3","result":{"type":"subscription_started"}}
{"event":"page_shown","data":{"type":"page_shown","left":{..},"right":{..}}}
```

The right-hand page is drawn by `herdfold reader attach`, an ordinary
client of this API, which passes its keys and mouse presses back with
`reader.send_keys` and `reader.send_mouse`. `note.add` writes a note into
the open book (`herdfold note add` is the same from a shell, which is how
an agent answers). Failures are `{"id":..,"error":{"code":..,"message":..}}`.
`herdfold api schema --json` prints the full schema; like herdr's own
commands, CLI failures are written to stderr as
`{"id":"cli:<group>:<command>","error":{..}}` with exit code 1.

## Install

```sh
brew install azihsoyn/tap/herdfold   # Homebrew (macOS/Linux)
cargo install herdfold               # or from crates.io
```

Or the prebuilt binary for macOS or Linux, from the
[latest release](https://github.com/azihsoyn/herdfold/releases/latest):

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/azihsoyn/herdfold/releases/latest/download/herdfold-installer.sh | sh
```

The spread and pictures need [herdr](https://herdr.dev); without it,
herdfold shows one page at a time, and pictures by their description.

## License

MIT or Apache-2.0, at your option.

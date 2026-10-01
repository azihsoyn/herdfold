# herdbook

Long text, laid out as facing pages you turn.

Scrolling has no sense of place; pages do. A page has a fixed amount on it, a
number that says where you are, and a turn that marks where one stretch of
reading ends and the next begins. `less`, `bat` and `ov` treat a long text as
an endless strip; this treats it as a book.

```sh
herdbook --format epub the-book.epub
herdbook --format md README.md
gh pr diff 123 | herdbook --format diff -
```

Inside [herdr](https://herdr.dev) the pane splits in two and the book opens
as a spread: left page in one pane, right page in the other, the text running
on from left to right. Elsewhere it shows one page at a time.

| Key | |
| --- | --- |
| `Space` / `→` | turn the page |
| `b` / `←` | turn back |
| `m` | bookmark what is open (again to remove) |
| `c` | change the colour of the bookmark here |
| `n` | write a note on the page |
| drag | choose text with the mouse, then `m` marker · `n` note · `?` ask · `y` copy |
| `v` | choose a row (`j` / `k`), then `Enter` to write a note on it, or `?` to ask about it |
| `?` | ask the agent about the open pages |
| `l` | bookmarks and notes: `Enter` to go, `d` to remove |
| `N` | notes as footnotes / in the margin / as marks only (remembered) |
| `g` | contents |
| `<` / `>` | shorter / longer rows (narrower / wider margins) |
| `a` | page-turn animation on / off (remembered) |
| `h` | list the keys |
| `T` | a tip (one greets each book until you tick "don't show again") |
| `q` / `Esc` | close the book (`Esc` closes the contents first, if open) |

## What it does, and what it doesn't

- **Pages are cut mechanically**, by the height of the pane. The text is never
  read for meaning.
- **Chapters open on a new page**, set a quarter of the way down, as a book
  sets its chapter openings. Only the top tier the input repeats counts (a
  lone `#` title over many `##` sections makes the sections the chapters),
  and a heading is never left alone at the foot of a page.
- **Rows are at most 72 columns**, with margins around them, however wide the
  pane. Typesetting has long held 60–80 characters to be readable. `<` and
  `>` change that by 4 columns at a time. The length is kept per book, and
  a book opened for the first time starts at the length last set in any
  book; `--measure COLS` sets it for one run.
- **Markdown is rendered**: headings drop their `#` and gain a rule, emphasis,
  code and links are styled, lists, quotes, code blocks and tables are set
  as such. Other formats are shown as they are.
- **Chapters come only from the input**: an EPUB's table of contents, Markdown
  headings, the files of a diff. Plain text has none, and none are guessed.
- **The format is always named** with `--format`; nothing is detected.
- **Notes are written in the book**: on a page (a pencil `✎` by the running
  head), or on a row (a stroke `▎` in the margin beside it). They are kept
  with the bookmarks, and listed with them under `l`. Their text is shown as
  footnotes at the foot of the page (the page then holds less, and the
  text runs on to the next), in the outer margin beside their row (when
  the margin is wide enough; `<` makes room), or not at all.
- **Text is chosen with the mouse**, within the page (or across both pages
  of a spread), and can be marked with a yellow highlighter, noted, asked
  about, or copied. The book takes the mouse for this, so the terminal's
  own selection, which in herdr would run across both panes, is not used.
- **The agent beside the book can be asked.** `?` sends the question, the
  open pages (and the chosen row, from `v`), to a herdr agent in the same
  tab (else the same workspace, or the one named with `--agent`). It
  answers in its own pane, and keeps a short answer in the book as a note
  (`✦`) by running `herdbook note add`.
- **A bookmark is a ribbon** hanging from the top of its page into the
  margin by the gutter, in one of six colours (`c` changes it; new
  bookmarks take the colour last chosen).
- **Your place is kept** per book in `~/.local/share/herdbook/marks.json`
  (`$XDG_DATA_HOME` if set), as a position in the text rather than a page
  number, so it survives a change of pane size.
- **There is no scrolling.** Only turning. A turn is drawn: the free edge of
  the sheet (`┃`) crosses the right page and then the left, the next pages
  appearing behind it, in about a third of a second. `a` switches this off
  (or back on) for every book from then on; `--no-animation` /
  `--animation` decide it for one run. Jumps from the contents are not
  animated.

`--no-spread` keeps one page at a time even inside herdr.

## Socket API

The reader speaks herdr's protocol: newline-delimited JSON over a Unix
socket, whose path it hands to the panes it opens as `HERDBOOK_SOCKET_PATH`.

```
{"id":"1","method":"ping","params":{}}
{"id":"1","result":{"type":"pong","version":"0.1.0","protocol":1}}

{"id":"2","method":"reader.send_keys","params":{"keys":["space"]}}
{"id":"2","result":{"type":"ok"}}

{"id":"3","method":"events.subscribe","params":{"subscriptions":[{"type":"page.shown"}]}}
{"id":"3","result":{"type":"subscription_started"}}
{"event":"page_shown","data":{"type":"page_shown","left":{..},"right":{..}}}
```

The right-hand pane passes its mouse presses on with `reader.send_mouse`.
`note.add` writes a note into the open book (`herdbook note add` is the same
from a shell, which is how an agent answers). Failures are
`{"id":..,"error":{"code":..,"message":..}}`. The right-hand
page is drawn by `herdbook reader attach`, an ordinary client of this API.
`herdbook api schema --json` prints the full schema; like herdr's own
commands, CLI failures are written to stderr as
`{"id":"cli:<group>:<command>","error":{..}}` with exit code 1.

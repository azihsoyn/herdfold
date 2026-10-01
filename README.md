# book

Long text, laid out as facing pages you turn.

Scrolling has no sense of place; pages do. A page has a fixed amount on it, a
number that says where you are, and a turn that marks where one stretch of
reading ends and the next begins. `less`, `bat` and `ov` treat a long text as
an endless strip; this treats it as a book.

```sh
book --format epub the-book.epub
book --format md README.md
gh pr diff 123 | book --format diff -
```

Inside [herdr](https://herdr.dev) the pane splits in two and the book opens
as a spread: left page in one pane, right page in the other, the text running
on from left to right. Elsewhere it shows one page at a time.

| Key | |
| --- | --- |
| `Space` / `→` | turn the page |
| `b` / `←` | turn back |
| `m` | bookmark what is open (again to remove) |
| `g` | contents: chapters, then bookmarks |
| `q` | close the book |

## What it does, and what it doesn't

- **Pages are cut mechanically**, by the height of the pane. The text is never
  read for meaning.
- **Rows are at most 72 columns**, with margins around them, however wide the
  pane. Typesetting has long held 60–80 characters to be readable.
- **Chapters come only from the input**: an EPUB's table of contents, Markdown
  headings, the files of a diff. Plain text has none, and none are guessed.
- **The format is always named** with `--format`; nothing is detected.
- **Your place is kept** per book in `~/.local/share/book/marks.json`
  (`$XDG_DATA_HOME` if set), as a position in the text rather than a page
  number, so it survives a change of pane size.
- **There is no scrolling.** Only turning.

Working name. The session building this picks the real one.

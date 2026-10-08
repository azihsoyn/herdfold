# A short book about books

## Pages, not a strip

Scrolling has no sense of place; pages do. A page holds a fixed amount, a
number says where you are, and a turn marks where one stretch of reading
ends and the next begins.[^turn]

A long text, a pull request, a book in EPUB or PDF: each is laid out as
facing pages across two panes, the text running on from the left page to
the right.

```rust
fn turn(&mut self) {
    self.page += 2; // a spread at a time
}
```

## Asking

A question about the pages in front of you goes to the agent beside the
book, with the pages themselves. It answers in its own pane, and leaves a
short answer in the book, where the question was asked.

Everything the book does is a call on its socket, so an agent can turn to
a chapter, find a passage, or hang a ribbon, just as the keys do.

## Ribbons and notes

A bookmark is a ribbon, hanging from the top of its page into the margin
by the gutter. A note goes on a page, on a row, or on a stretch of text
chosen with the mouse. Each keeps the words it is on, and finds them again
if the book changes.

## Books bound on the right

Japanese books and manga run the other way: the right page first, turns
sweeping from left to right, and the lines set in columns.

## Notes

[^turn]: A turn is drawn: the free edge of the sheet crosses the right page
    and then the left, the next pages appearing behind it.

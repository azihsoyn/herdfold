# Regenerating demo.gif

The demo runs the real `herdfold` inside a herdr session of its own,
recorded by [vhs](https://github.com/charmbracelet/vhs). From the
repository root:

```sh
cargo build --release
vhs demo/demo.tape
XDG_CONFIG_HOME=/tmp/hfd/cfg herdr session stop hfdemo
```

`demo/setup.sh` (run by the tape) prepares `/tmp/hfd`: a home, config and
data of the demo's own, so nothing of yours shows; a shell whose prompt is
only `$ ` (herdr starts the pane's shell as a login shell, which would
otherwise show your user and host); and a copy of `demo/book.md`, a short
book written for the demo, to read. The
path is kept short because herdr's socket lives under it, and Unix socket
paths are limited to about 104 bytes.

Two things are not in the demo. Mouse selection: vhs cannot drive a
mouse. Pictures: herdr sets them through the terminal's graphics protocol,
which the terminal vhs records in does not draw, so `book.md` has none.

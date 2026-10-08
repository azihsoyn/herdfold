# The README demo, three books across two herdr panes: a Markdown book
# whose footnote is followed and come back from, Rashomon set in vertical
# columns and turned right to left, and a change to herdfold with its code
# highlighted. Regenerate from the repository root with:
#   cargo build --release && vhs demo/demo.tape
#   XDG_CONFIG_HOME=/tmp/hfd/cfg herdr session stop hfdemo
# It runs a herdr session of its own (hfdemo) under /tmp/hfd, prepared by
# demo/setup.sh, with a bare shell and no config of yours.
Output demo.gif

Set FontSize 15
Set Width 1600
Set Height 900
Set Theme "Catppuccin Mocha"
Set TypingSpeed 45ms
Set Shell "bash"

Env HOME "/tmp/hfd/home"
Env SHELL "/tmp/hfd/bin/shell"
Env PS1 "$ "
Env XDG_CONFIG_HOME "/tmp/hfd/cfg"
Env XDG_DATA_HOME "/tmp/hfd/data"
Env XDG_CACHE_HOME "/tmp/hfd/cache"

Hide
Type "export PATH=$PWD/target/release:/opt/homebrew/bin:$PATH; unset HERDR_ENV HERDR_PANE_ID HERDR_TAB_ID HERDR_WORKSPACE_ID HERDR_SOCKET_PATH"
Enter
Type "sh demo/setup.sh && cd /tmp/hfd/herdfold && herdr --session hfdemo"
Enter
Sleep 5s
# herdr's first-run welcome, then its integrations screen
Enter
Sleep 1500ms
Escape
Sleep 1s
Type "clear"
Enter
Sleep 500ms
Show

Sleep 800ms
Type "herdfold --format md book.md"
Sleep 400ms
Enter
Sleep 3.5s
# A footnote: point at it, see where it leads, go there, come back
Type "f"
Sleep 1.8s
Enter
Sleep 2.8s
Enter
Sleep 2.2s
Backspace
Sleep 2.2s
Space
Sleep 2.2s
Type "q"
Sleep 1.2s
# Rashomon, from Aozora Bunko: set in vertical columns, turned right to left
Type "herdfold --format aozora rashomon.txt"
Sleep 400ms
Enter
Sleep 3.5s
Type "V"
Sleep 3s
Space
Sleep 2.5s
Space
Sleep 3s
Type "q"
Sleep 1.2s
# A pull request's change, each file a chapter, its code highlighted
Type "herdfold --format diff change.diff"
Sleep 400ms
Enter
Sleep 3.5s
Space
Sleep 3s
Type "q"
Sleep 1.2s

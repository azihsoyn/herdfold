# The README demo: a spread across two herdr panes, turned, bookmarked,
# noted, searched. Regenerate from the repository root with:
#   cargo build --release && vhs demo/demo.tape
# It runs a herdr session of its own (hfdemo) under /tmp/hfd, prepared by
# demo/setup.sh, with a bare shell and no config of yours.
Output demo.gif

Set FontSize 14
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

Hide
Type "export PATH=$PWD/target/release:/opt/homebrew/bin:$PATH; unset HERDR_ENV HERDR_PANE_ID HERDR_TAB_ID HERDR_WORKSPACE_ID HERDR_SOCKET_PATH"
Enter
Type "sh demo/setup.sh && cd /tmp/hfd/herdfold && herdr --session hfdemo"
Enter
Sleep 5s
# herdr's first-run welcome
Enter
Sleep 1s
Type "clear"
Enter
Sleep 500ms
Show

Sleep 800ms
Type "herdfold --format md README.md"
Sleep 400ms
Enter
Sleep 3.5s
# Put the tip away, and turn
Enter
Sleep 1.2s
Space
Sleep 1.5s
Space
Sleep 1.5s
# A ribbon, and its colour
Type "m"
Sleep 1.2s
Type "c"
Sleep 1.2s
# A note on the page
Type "n"
Sleep 500ms
Type "Worth reading twice."
Sleep 500ms
Enter
Sleep 2s
# Search, step, and the list of finds
Type "/"
Sleep 400ms
Type "agent"
Sleep 1s
Enter
Sleep 600ms
Type "n"
Sleep 1.2s
Type "n"
Sleep 1.2s
Type "l"
Sleep 1.8s
Down
Sleep 500ms
Down
Sleep 500ms
Enter
Sleep 1.5s
Escape
Sleep 800ms
# Contents
Type "g"
Sleep 1.5s
Down
Sleep 500ms
Enter
Sleep 1.8s
# The keys
Type "h"
Sleep 2.5s
Type "q"
Sleep 800ms
Type "q"
Sleep 1.2s

# The README demo: a spread across two herdr panes, turned, the contents
# drawer folded, a ribbon hung, a note kept, a search stepped through, the
# book turned round to read right to left. Regenerate from the repository
# root with:
#   cargo build --release && vhs demo/demo.tape
#   XDG_CONFIG_HOME=/tmp/hfd/cfg herdr session stop hfdemo
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
# Put the tip away, and turn
Enter
Sleep 1.2s
Space
Sleep 1.6s
Space
Sleep 1.6s
# The contents: fold a chapter's sections, open them, go to another
Type "g"
Sleep 1.5s
Up
Sleep 350ms
Up
Sleep 350ms
Up
Sleep 350ms
Up
Sleep 350ms
Up
Sleep 800ms
Left
Sleep 1.2s
Right
Sleep 1.2s
Down
Sleep 600ms
Enter
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
Type "ribbon"
Sleep 1s
Enter
Sleep 600ms
Type "n"
Sleep 1.2s
Type "l"
Sleep 1.8s
Down
Sleep 500ms
Enter
Sleep 1.5s
Escape
Sleep 800ms
# Turned round: pages that run right to left
Type "D"
Sleep 1.5s
Space
Sleep 1.8s
Type "D"
Sleep 1.2s
# The keys
Type "h"
Sleep 2.5s
Escape
Sleep 800ms
Type "q"
Sleep 1.2s

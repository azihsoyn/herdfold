#!/bin/sh
# Prepares /tmp/hfd for demo.tape: a home, config and data of its own (so
# nothing of yours shows), a shell that prints only "$ " (herdr starts the
# pane's shell as a login shell, which would read /etc/bashrc and show the
# user and host), no tips (they would greet each book), and the books to
# read: demo/book.md, demo/rashomon.txt (Akutagawa's Rashomon, from Aozora
# Bunko, in the public domain) and demo/change.diff (a change to herdfold).
set -e
rm -rf /tmp/hfd
mkdir -p /tmp/hfd/home /tmp/hfd/cfg /tmp/hfd/data /tmp/hfd/bin /tmp/hfd/herdfold
printf 'PS1="$ "\n' > /tmp/hfd/bashrc
printf '#!/bin/sh\nexec /bin/bash --noprofile --rcfile /tmp/hfd/bashrc -i\n' > /tmp/hfd/bin/shell
chmod +x /tmp/hfd/bin/shell
mkdir -p /tmp/hfd/data/herdfold
printf '{"tips":false}\n' > /tmp/hfd/data/herdfold/settings.json
cp demo/book.md demo/rashomon.txt demo/change.diff /tmp/hfd/herdfold/

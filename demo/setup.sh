#!/bin/sh
# Prepares /tmp/hfd for demo.tape: a home, config and data of its own (so
# nothing of yours shows), a shell that prints only "$ " (herdr starts the
# pane's shell as a login shell, which would read /etc/bashrc and show the
# user and host), and the book to read.
set -e
rm -rf /tmp/hfd
mkdir -p /tmp/hfd/home /tmp/hfd/cfg /tmp/hfd/data /tmp/hfd/bin /tmp/hfd/herdfold
printf 'PS1="$ "\n' > /tmp/hfd/bashrc
printf '#!/bin/sh\nexec /bin/bash --noprofile --rcfile /tmp/hfd/bashrc -i\n' > /tmp/hfd/bin/shell
chmod +x /tmp/hfd/bin/shell
cp README.md /tmp/hfd/herdfold/

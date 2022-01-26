#!/bin/bash

set -o errexit
set -o nounset
set -o pipefail
set -o errtrace

main () {
    # The destination is a Docker mounted volume so we postpone initialization until the startup of the entire container, when the volume is already mounted.
    radicle-service-init --root /app/radicle/root --identity /app/radicle/identity
    cp --force /usr/local/bin/pre-receive /app/radicle/root/git/hooks/pre-receive
    cp --force /usr/local/bin/post-receive /app/radicle/root/git/hooks/post-receive

    exec /usr/local/bin/radicle-git-server --git-receive-pack --allow-unauthorized-keys --root /app/radicle/root --listen 0.0.0.0:8778
}

main "$@"

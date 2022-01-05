#!/bin/bash

set -o errexit
set -o nounset
set -o pipefail
set -o errtrace

main () {
    # The destination is a Docker mounted volume so we postpone copying until the startup of the entire container, when the volume is already mounted.
    mkdir --parents /app/radicle/git/hooks
    cp --force /usr/local/bin/pre-receive /app/radicle/git/hooks/pre-receive

    exec /usr/local/bin/radicle-git-server --git-receive-pack --identity /app/radicle/identity --root /app/radicle/root --listen 0.0.0.0:8778
}

main "$@"

#!/bin/bash

set -o errexit
set -o nounset
set -o pipefail
set -o errtrace

main () {
    # The destination is a Docker mounted volume so we postpone copying until the startup of the entire container, when the volume is already mounted.
    radicle-service-init --root /app/radicle --identity /app/radicle/identity

    mkdir --parents /app/radicle/git/hooks
    cp --force /usr/local/bin/pre-receive /app/radicle/git/hooks/pre-receive
    cp --force /usr/local/bin/post-receive /app/radicle/git/hooks/post-receive

    exec /usr/local/bin/radicle-git-server --git-receive-pack --root /app/radicle/root --listen 0.0.0.0:8778
}

main "$@"

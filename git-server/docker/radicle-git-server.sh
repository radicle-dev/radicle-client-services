#!/bin/bash

set -o errexit
set -o nounset
set -o pipefail
set -o errtrace

main () {
    rad_profile=$(rad self --profile)
    cp --force /usr/local/bin/pre-receive /app/radicle/$rad_profile/git/hooks/pre-receive
    cp --force /usr/local/bin/post-receive /app/radicle/$rad_profile/git/hooks/post-receive
    exec /usr/local/bin/radicle-git-server "$@"
}

main "$@"

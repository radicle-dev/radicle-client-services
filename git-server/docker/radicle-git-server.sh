#!/bin/bash

set -o errexit
set -o nounset
set -o pipefail
set -o errtrace

main () {
    if [[ -z ${RAD_HOME:-} ]]; then
        echo "RAD_HOME is unset"
        return 1
    fi
    rad_profile=$(cat "${RAD_HOME}/active_profile")
    cp --force /usr/local/bin/pre-receive "${RAD_HOME}/${rad_profile}/git/hooks/pre-receive"
    cp --force /usr/local/bin/post-receive "${RAD_HOME}/${rad_profile}/git/hooks/post-receive"
    exec /usr/local/bin/radicle-git-server "$@"
}

main "$@"

#!/bin/bash

set -o errexit
set -o nounset
set -o pipefail
set -o errtrace
export SHELLOPTS

main () {
    influx user create --name readonly --password readonly
    local this_script_path=$(realpath $0)
    local this_script_dir=$(dirname "$this_script_path")
    influx apply --force true --file "${this_script_dir}/template.yaml"
}

main "$@"

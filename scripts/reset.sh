#!/bin/sh

set -e

ROOT=${1:-root}

rm -rf $ROOT
cargo run -p radicle-service-init -- --root $ROOT --identity $ROOT/identity
cp target/debug/{pre,post}-receive $ROOT/git/hooks
cp scripts/post-receive-ok $ROOT/git/hooks
cp authorized-keys $ROOT/git/

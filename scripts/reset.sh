#!/bin/sh
confirm() {
  read -p "$1 [y/n] " choice
  case "$choice" in
    y|Y) ;;
    *) exit 1 ;;
  esac
}

export LNK_HOME=${1:-root}

### Delete old identity ###

MONOREPO=$(rad path 2>/dev/null)
RESULT=$?

set -e

if [ $RESULT -eq 0 ]; then
  echo "Identity exists..."
  if [ -d $MONOREPO ]; then
    confirm "Delete $MONOREPO?"
    rm -rf $MONOREPO
  fi
fi

### Initialize new identity ###

rad auth --init --name "seed" --passphrase "seed"
echo
echo "Initialized $(rad path)"

MONOREPO=$(rad path)

set -x
cp target/debug/{pre,post}-receive $MONOREPO/hooks
cp scripts/post-receive-ok $MONOREPO/hooks
cp authorized-keys $MONOREPO/

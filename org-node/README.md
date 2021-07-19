# Radicle Org Node

> ✨ Host your Radicle Org projects!

## Building

    $ cargo build --release

## Installing

    $ cargo install

## Running

Generate a key, eg. with `radicle-keyutils`, then run the org node like this,
for example:

    $ radicle-org-node \
      --root ~/.radicle \
      --subgraph 'https://api.thegraph.com/subgraphs/name/radicle-dev/radicle-orgs' \
      --orgs 0x5C70e249529e7D43f68E5c566e489937559f73E5 \
      --identity ~/.secret.key \
      --bootstrap 'hynkyndc6w3p8urucakobzna7sxwgcqny7xxtw88dtx3pkf7m3nrzc@sprout.radicle.xyz:12345'

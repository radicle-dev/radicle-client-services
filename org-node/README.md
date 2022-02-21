# Radicle Org Node

> ✨ Host your Radicle Org projects!

The *org node* service is a peer-to-peer service which uses the Radicle Link protocol
to connect to peers and replicate projects under the specified org(s). To find
which projects to replicate, it listens for events and updates on the Ethereum
network where Radicle orgs keep their state.

When a new project is added to an org via a process called *anchoring*, the org
node attempts to fetch this project from the network.

> 💡 It is recommend to configure your Radicle clients to use your org node(s) as seeds,
so that changes you make to projects as well as projects you create are made available
to the org node with minimal delay, and thus the rest of the network.

Though the org node helps with reliable code distribution over the Radicle Link
network, it does not expose projects in a way that is accessible to web and other
HTTP clients. This is what the `radicle-http-api` is for. To serve `git` clients,
the `radicle-git-server` should be used.

#### Bootstrapping

It's generally useful for the org seed node to connect to a pre-existing
node to replicate projects from and find peers. This is done via the
`--bootstrap` flag. See `radicle-org-node --help` for details on the format.

Any (seed) node will do as a bootstrap peer. Multiple bootstrap nodes may
be specified by separating them with a `,`.

#### JSON-RPC URL

For `radicle-org-node`, it's necessary to specify a WebSocket URL to an
Ethereum full node with JSON-RPC and WebSocket support, using the `--rpc-url`
option.  This could be the address to your own node running locally, eg.
`ws://localhost:8545`, or the URL of a third-party API such as Alchemy or
Infura.

## Building

    $ cargo build --release

## Installing

    $ cargo install

## Running

For example:

    $ radicle-org-node \
      --root ~/.radicle \
      --subgraph 'https://api.thegraph.com/subgraphs/name/radicle-dev/radicle-orgs' \
      --orgs 0x5C70e249529e7D43f68E5c566e489937559f73E5 \
      --identity ~/.radicle/secret.key \
      --bootstrap 'hynkyndc6w3p8urucakobzna7sxwgcqny7xxtw88dtx3pkf7m3nrzc@sprout.radicle.xyz:12345' \
      --rpc-url ws://localhost:8545

## Docker

To run the org node via docker, you can run for eg.

    $ docker run \
        --init \
        -e RUST_LOG=info \
        -p 8776:8776 \
        -v $HOME/.radicle:/app/radicle radicle-services/org-node \
        --subgraph https://api.thegraph.com/subgraphs/name/radicle-dev/radicle-orgs \
        --orgs 0xceAa01bd5A428d2910C82BBEfE1Bc7a8Cc6207D9 \
        --rpc-url ws://localhost:8545

Make sure the *identity* file can be found under `$HOME/.radicle/identity` and
that you replace the org address with your own. Don't forget to specify the
`--bootstrap` option if needed.

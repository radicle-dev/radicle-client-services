# Radicle Client Services

🏕️ Services backing the Radicle client interfaces.

## Setting up an *Org Seed Node*

An *org seed node* is a type of node that replicates and distributes Radicle
projects under one or more Radicle org, making them freely and publicly
accessible on the web.

Though it's possible to rely on shared infrastructure and community seed nodes,
it is recommend for most orgs (and users) to self-host their projects in true
peer-to-peer fashion. This can be achieved by running `radicle-org-node` and
optionally `radicle-http-api` on a server or instance in the cloud.

### `radicle-org-node`

The *org node* service is a peer-to-peer service which uses the Radicle Link protocol
to connect to peers and replicate projects under the specified org(s). To find
which projects to replicate, it listens for events and updates on the Ethereum
network where Radicle orgs are hosted. When a new project is added to an org via
a process called *anchoring*, the org node attempts to fetch this project
from the network.

It is recommend to configure your Radicle client to use your org node(s) as seeds,
so that changes you make to projects as well as projects you create are made available
to the org node with minimal delay.

Though the org node helps with reliable code distribution over the Radicle Link
network, it does not expose projects in a way that is accessible to web and other
HTTP clients. This is what the `radicle-http-api` is for.

### `radicle-http-api`

The Radicle HTTP API is a lightweight HTTP service that runs on top of a Radicle
*monorepo*. As a reminder, the "monorepo" is the repository that contains all
projects and associated metadata stored and replicated by Radicle Link.

By running the API, projects stored in the local monorepo are exposed via a
JSON HTTP API. This can enable clients to query project source code directly
via HTTP, without having to run a node themselves. In particular, the Radicle
web client was built around this API.

### Service setup

Though it's possible to only run the `radicle-org-node`, for maximum accessibility,
it's recommended to run both services. Since the HTTP API wraps a local monorepo,
these services should have access to the same file-system. The org node requires
*write* access to the file system, while the HTTP API only requires *read* access.

For this setup to work, it's import to point both services to the same *root*,
which is the path to the monorepo, eg.:

    $ radicle-org-node --root ~/.radicle/root --orgs …
    $ radicle-http-api --root ~/.radicle/root …

This ensures the API can read the org node's state.

#### Firewall configuration

For `radicle-org-node`, a UDP port of your choosing should be opened. This port
can then be specified via the `--listen` parameter, eg.
`radicle-org-node --listen 0.0.0.0:8776`.  The default port is `8776`.

For `radicle-http-api`, an HTTP port of your choosing should be opened. This port
can then be specified via the `--listen` parameter, eg.
`radicle-http-api --listen 0.0.0.0:8777`.  The default port is `8777`.

#### TLS

For `radicle-http-api`, it's important to setup TLS when running in production.
This is to allow for compatibility with web clients that will mostly be using
the `https` protocol. Web browser nowadays do not allow requests to unencrypted
HTTP servers from websites using TLS.

#### Logging

To enable logging for either service, set the `RUST_LOG` environment variable.
Setting it to `info` is usually enough, but `debug` is also possible.

### Org setup

Once these services are running, orgs wishing to point Radicle clients to them
for project browsing should set the relevant records on ENS. This requires
an ENS name to be registered for each org.

To point Radicle Link clients to the right seed endpoint, use the `eth.radicle.seed.id`
TXT record, usually labeled "Seed ID" to your Seed ID, eg.

    hynkyndc6w3p8urucfkobzna7sxbgctny7xxtw88dtx3pkf7m3nrzc@seed.acme.org:8776

To point HTTP clients to the right service endpoint, use the `eth.radicle.seed.api`
TXT record, usually labeled "Seed API" to your HTTP API endpoint, eg.

    https://seed.acme.org:8777

These records can be set on the web client at <https://app.radicle.network/registrations/acme>
for an org registered under `acme.radicle.eth`. Replace `acme` with the name
of your org.

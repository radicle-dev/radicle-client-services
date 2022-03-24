# Radicle Client Services

🏕️ Services backing the Radicle client interfaces.

## Setting up a *Seed Node*

A *seed node* is a type of node that replicates and distributes Radicle
projects making them freely and publicly accessible on the web, and via
peer-to-peer protocols.

Though it's possible to rely on shared infrastructure and community seed nodes,
it is recommend for most teams and communities to self-host their projects in true
peer-to-peer fashion. This can be achieved by running `radicle-http-api` and
`radicle-git-server` on a server or instance in the cloud.

### `radicle-http-api`

The Radicle HTTP API is a lightweight HTTP service that runs on top of a Radicle
*monorepo*. As a reminder, the "monorepo" is the repository that contains all
projects and associated metadata stored and replicated by Radicle Link.

By running the API, projects stored in the local monorepo are exposed via a
JSON HTTP API. This can enable clients to query project source code directly
via HTTP, without having to run a node themselves. In particular, the Radicle
web client was built around this API.

### `radicle-git-server`

The Radicle Git Server is an HTTP server that can serve repositories managed
by Radicle. Since Radicle projects are stored in a shared, monolithic repository,
commands like `git clone` cannot work out of the box. The Radicle Git Server
maps requests to specific namespaces in the shared repository and allows a Radicle
node to act as a typical Git server. It is then possible to clone a project
by simply specifying its ID, for example:

    git clone https://seed.alt-clients.radicle.xyz/hnrkyghsrokxzxpy9pww69xr11dr9q7edbxfo.git

### Service setup

The services should have access to the same file-system. The git server requires
*write* access to the file system, while the HTTP API only requires *read* access.

For this setup to work, it's import to point all services to the same *root*,
which is the path to the monorepo, eg.:

    $ radicle-http-api --root ~/.radicle/root …
    $ radicle-git-server --root ~/.radicle/root …

This ensures the API and Git server can read the same state.

#### Identity file

Nodes on the Radicle peer-to-peer network are identified with a *Peer ID*,
which is essentially an encoding of a public key. This identity needs to
be specified on the CLI via the `--identity` flag, similar to SSH's `-i`
flag. Specifically, the path to the private key file should be used. If
no private key is found at that path, a new key will be generated.

#### Firewall configuration

For `radicle-http-api`, an HTTP port of your choosing should be opened. This port
can then be specified via the `--listen` parameter, eg.
`radicle-http-api --listen 0.0.0.0:8777`.  The default port is `8777`.

For `radicle-git-server`, it is recommended that port `443` be open.

#### TLS

For `radicle-http-api` and `radicle-git-server`, it's important to setup TLS
when running in production.  This is to allow for compatibility with web
clients that will mostly be using the `https` protocol. Web browser nowadays do
not allow requests to unencrypted HTTP servers from websites using TLS.

The API service has built-in support for TLS, so there is no need to set up
HTTPS termination via a separate service. Simply pass in the `--tls-cert`
and `--tls-key` flags to enable TLS.

Certificates can be obtained from *Let's Encrypt*, using [Certbot](https://certbot.eff.org/).

#### Logging

To enable logging for either service, set the `RUST_LOG` environment variable.
Setting it to `info` is usually enough, but `debug` is also possible.

### ENS setup

Once these services are running, users wishing to point Radicle clients to them
for project browsing should set the relevant records on ENS. This requires
an ENS name to be registered for each seed node.

To point Radicle clients to the right seed endpoint, use the
`eth.radicle.seed.host` text record, usually labeled "Seed Host" to the
seed host, eg `seed.acme.org`.

These records can be set on the web client. For example, the records for the
Alt.-clients org can be found at <https://app.radicle.network/registrations/alt-clients.radicle.eth>.

### Docker

There are `Dockerfile` provided for both services in the respective directories.

#### Building

To build the containers, run:

    $ docker build -t radicle-services/http-api -f http-api/Dockerfile .
    $ docker build -t radicle-services/git-server -f git-server/Dockerfile .

#### Running

To run the HTTP API, run:

    $ docker run \
        --init \
        -e RUST_LOG=info \
        -p 8777:8777 \
        -v $HOME/.radicle:/app/radicle radicle-services/http-api \
        --tls-cert /app/radicle/fullchain.pem \
        --tls-key /app/radicle/privkey.pem

Make sure your TLS certificate files can be found under `$HOME/.radicle`. If you
are not using TLS termination, simply omit the `--tls-*` arguments.

Running `radicle-git-server` is more or less identical to running the HTTP API.

You may also want to detach the process (`-d`) and run with a TTY in interactive
mode (`-it`).

### Docker Compose

As an alternative to building the containers yourself, a `docker-compose.yml`
file is included in the repository. To run the services via Docker Compose, you
have to:

1. Install Docker and Docker Compose
2. Clone this repository
3. Set the necessary environment variables
4. Start the radicle client services via Docker Compose

To install Docker Compose, run:

    sudo apt-get install docker
    pip install docker-compose

Then clone this repository and `cd` into it:

    git clone <repository-url> radicle-client-services
    cd radicle-client-service

Then set `RADICLE_DOMAIN` to your seed node's domain, eg. `seed.cloudhead.io`.
These can be set in the environment, or in a `.env` file in the current
directory.

Finally, pull the containers and start the services:

    docker-compose pull
    docker-compose up --detach


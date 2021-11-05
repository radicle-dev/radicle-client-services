# Design of the radicle client services

The radicle client services are a set of networked daemons that offer data
availability, code distribution and repository hosting for Radicle communities.

They are designed to work in tandem with each other, each providing a specific
functionality, such as HTTP access, Git support or Ethereum integration.

Each organization, team, community or user can deploy the client services to
participate in the radicle network.  They are the backbone of the radicle
network.  For the purposes of this document, we shall call such a deployment a
*seed*, and use the terms "community", "org" and "team" interchangeably.

Seeds are configured to track radicle projects via various mechanisms.
Currently, two mechanisms are supported: Ethereum-based tracking (requires a
radicle org), and URN-based. The former works by listening to events
occurring in smart contracts on chain, while the latter allows the operator of
the seed to specify which projects should be tracked by providing a list of URNs.

Projects that are tracked by a seed will be fetched from the network and replicated
by the seed, as well as served over HTTP and Git.

Once deployed, a seed can serve its purpose of hosting code and providing API access
to the chosen repositories, but for it to be easily discoverable and usable by
client applications, it's recommended to register an ENS name.

By registering an ENS name, for example under the `.radicle.eth` domain, a
community can specify the follow things:

* A profile, with for eg. a name, avatar, website and twitter handle
* A seed host, which associates a physical seed address to the community
* A seed ID, which specifies the public network identity of the associated seed
* An anchors address, which tells clients where to look for project anchors

For now, radicle orgs are the only mechanism supported for storing project anchors,
which are `(project-id, project-commit-hash)` tuples that represent the community's
shared understanding of the state of a project. In the future, it will be possible
to anchor projects on Layer 2 as well as on other mediums.

Although seeds are compatible with the radicle link peer-to-peer replication
protocol, they also have their own mechanism for sharing and distributing code.
Seeds come with a *git bridge* which sits in front of the radicle link state and
offers read and write git access without any special tooling.

Read and write access are offered via the Git HTTP backend, using GPG keys for
authentication.  Namely, signed git pushes are used for writing to the seed
node state.  The set of keys authorized to push to a project can be stored in
the repository itself, under the `.rad/keys` directory. Other mechanisms for
configuring authorized keys, eg. using Ethereum addresses or Radicle IDs are
in the works.

Pushes to the seed end up under the seed's local project tree, which is offered
to the network along with other trees, and is signed by the seed's private key.

While the Git bridge offers git access to the project state, the HTTP API offers
a RESTful JSON API usable by web clients.

## Compared to Radicle Link

Though the client services interoperate with Link, by allowing read and write
access to the state, and participating in the gossip network, they offer an
additional method of distribution which has one basic advantage: no special
tooling is needed to push or pull code from a seed. Only a recent version of
Git is required. This promises to reduce friction for new users, or users
outside of the radicle network.

Since the seed node participates in the peer to peer network, it does not
represent a single point of failure for the community or team, when it comes
to repository access. If the seed goes down, users can fallback to the peer-to-peer
network. Hence, we see the client services as an additional method of distribution
on top of Link.

## Compared to self-hosted forges

Certain forges offer self-hosting, for example GitLab or Gitea. This allows
communities to run their own instance of the platform on their servers.
Self-hosted forges have a major drawback, which is that they require users
to create accounts on each of the instances, and there is no global social
layer: users of different instances cannot interact with each other on the
same website or platform. Each instance has a completely closed and isolated
user base. There is no way to build an identity across multiple projects and
communities. Furthermore, project discovery is complicated due to there being
no possibility of a shared database of projects.


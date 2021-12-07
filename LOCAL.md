# Running locally

You might need to run a seed node locally to experiment or for development purposes. Here's how you can get set up.

## Insall `rad`

Install `rad` binary from [radicle-link](https://github.com/radicle-dev/radicle-link/tree/master/bins/rad) repo. You need it to create identities for yourself, and the projects you'll be creating.

All your data would live in a monorepo (root), so designate a folder for it and then set:

```
$ export RAD_HOME=/path/to/folder
```

Then (replace values between `< >` with your own):

```
$ rad profile create
please enter your passphrase: (leave empty)
profile id: <profile-id>
peer id: <peer-id>

$ rad profile ssh add
please enter your passphrase: (press enter)
added key for profile id `<profile-id>`

$ rad identities person create new --payload '{"name": "<username>"}'
{"urn":"<urn>","payload":{"https://radicle.xyz/link/identities/person/v1":{"name":"<username>"}}}

$ rad identities local set --urn <ur>
set default identity to `<urn>`

# your git project should be at /path/to/working-dir/<project-name>/
$ rad identities project create existing --path /path/to/working-dir --payload '{"name": "<project-name>", "default_branch": "master"}'
{"urn":"<project-urn>","payload":{"https://radicle.xyz/link/identities/project/v1":{"name":"<project-name>","description":null,"default_branch":"master"}}}
```

Now create another env variable pointing to the profile you created in first step:
```
$ ls $RAD_HOME
<profile-id>/  active_profile

$ export LOCAL_ROOT=$RAD_HOME/<profile-id>/
```

All set.

## `org-node`

You can now run `org-node` with:

```
$ target/debug/radicle-org-node --subgraph https://api.thegraph.com/subgraphs/name/radicle-dev/radicle-orgs --rpc-url wss://eth-rinkeby.alchemyapi.io/v2/<token> --root $LOCAL_ROOT --identity $LOCAL_ROOT/keys/librad.key --identity-passphrase ''
```

## `git-server`

For a fully working [`git-server`](https://github.com/radicle-dev/radicle-client-services/tree/master/git-server) you'd need to also compile `pre-receive` and `post-receive` binaries and copy them into:

```
$ cp target/debug/{pre,post}-receive $LOCAL_ROOT/git/hooks/
```

These binaries are responsible for authentication which is through GPG keys, make sure you have one and:

```
$ gpg --list-keys --keyid-format=long
pub   rsa3072/<gpg-pub> 2020-10-10 [SC] [expires: 2023-10-10]
...
```

Finally you can run:

```
$ target/debug/radicle-git-server --root $LOCAL_ROOT --git-receive-pack --authorized-keys <gpg-pub>
```

Which will accept signed pushes (using `git push --signed`) from you and reject all else. To simplify your workflow you can add your key locally to your project as well using [`rad-auth-keys`](https://github.com/radicle-dev/radicle-client-tools):

```
$ gpg --armor --export <gpg-pub> | rad-auth-keys add
```

If you have trouble with `gpg` and `git` you can also run:

```
$ GIT_TRACE=1 git ...
```

To determine where the problem lies, e.g. a silent issue I was encountering was not having permission to access `secring.gpg`:

```
$ sudo chown user:user ~/.gnupg/secring.gpg
```

## `http-api`

Similar to `git-server` but with less parameters:

```
$ target/debug/radicle-http-api --root $LOCAL_ROOT
```


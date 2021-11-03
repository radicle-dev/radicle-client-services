# Radicle Git Server

> ✨ Serve Radicle Git repositories via HTTP

# Running

    $ radicle-git-server --root ~/.radicle

# Git Hooks

Git [hooks](https://git-scm.com/book/en/v2/Customizing-Git-Git-Hooks) are used by the git http backend to manage requests made to a repository, such as a `push` action. Hooks are executable files that accept standard input, perform some action and return an exit status back to the sender of the request, either successfully completing the request or declining.

This crate includes a `bin` folder that contains hooks used by the `radicle-git-server` for authorizing requests and performing other tasks.

In order to use these hooks, the binaries in the `bin` folder must be built using `cargo build --bin pre-receive --feature hooks` and `cargo build --bin post-receive --feature hooks` (`--release` for production) and moved to the radicle root under `git/hooks/` (e.g. `~/.radicle/git/hooks/`) folder. These executables will automatically be called by the http-backend.

## Authorizing Signed Push Certificates in `pre-receive` Hook

The `pre-receive` hook is the first hook invoked by the git http-backend when a `receive-pack` event is triggered from a `git push` client request.

The `pre-receive` hook is used to verify the git signed push certificates from the sender, e.g. `git push --signed ...`. Unless the `git-server` is run with the `--allow-unauthorized-keys` flag, any unsigned git push will be denied by the `pre-receive` hook.

### Allow Unauthorized Keys

While developing it can sometimes be useful to disable certificate verification and key authorization. To disable certificate checking, run the `git-server` with the following command:

```
radicle-git-server ... --allow-unauthorized-keys
```

### Providing a Command Line Argument Authorized Keyring

It is possible to provide the `git-server` with a comma delimited list of authorized GPG key fingerprints to use as a simple keyring for verifying a signed push certificate. To set an authorized keys list, run the `git-server` with the following command:

```
radicle-git-server ... --authorized-keys 817EEFE32E1F0AA5,...,...
```

### Using `.rad/keys/openpgp/<key_id>` for Authorization

By default, the `pre-receive` hook will check the target repository for a `.rad/keys/openpgp/<key_id>` public key file on push. If it exists, it will check the public key's fingerprint matches the `$GIT_PUSH_CERT_KEY` set by the http-backend. The `$GIT_PUSH_CERT_KEY` is used to find the file in the namespace tree, comparing the fingerprint in the authorized keyring against the signed certificate.

No `git-server` command arguments are needed to perform this check.

In order to setup your `.rad/keys/` keyring, there is a CLI tool, `rad-auth-keys`, in `radicle-client-tools/authorized-keys` that provides helper commands for exporting your gpg key and placing it into your `.rad/keys/` keyring.

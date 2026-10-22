# Contacts Spec

The contacts directory (`Records/contacts/<id>.md`) and the CardDAV accounts
that fill it from Nextcloud, iCloud or any CardDAV server. This spec is about
the credential those accounts carry: an app-specific password that unlocks a
person's whole address book, and on iCloud more than that. It must reach the
CardDAV server it was issued for and nothing else — not the vault, not a peer,
not another host.

Implementation: `features/contacts/contacts` (`credentials`, `carddav`,
`vault_contacts`), built per org by `task-server`'s `build_org_state`, which
every node runs. The node-private directory the accounts live in is
`vault.private.node-only` in [`../../vault/spec/vault.md`](../../vault/spec/vault.md).

---

## The credential

### The password is never in the vault

t[contacts.secret.out-of-vault]
An account file holds connection details and a reference to its password,
never the password. The password is kept by a credential store — the OS
keyring where it is durable, otherwise the `auth::crypto` envelope keyed by
the node's auth secret — and resolved only at sync time. Listing accounts
returns the password blank. A reference to a test-only source is never
resolved by a production store. Opening a vault that still holds plaintext
from before this rule moves it to the store, or removes it and marks the
account for re-entry; plaintext never survives an open. A restart resolves
what the previous process stored.

---

## The transport

### https only

t[contacts.transport.https-only]
An account whose server (or addressbook) URL is not `https` — or carries
credentials in the URL — is refused when it is saved, and again when it is
synced, so a file edited by hand gets the same answer. There is no plain-http
transport to fall back to.

---

### Credentials stay on the account's own server

t[contacts.transport.same-origin-credentials]
The `Authorization` header is sent only to the account's credential scope: the
configured origin over https (for iCloud, Apple's `icloud.com` hosts on 443,
because iCloud serves an address book from a numbered partition host).
Redirects are followed by hand and only within that scope; a redirect or an
href that leaves it is refused before any request is made, so another host
receives no request carrying the credential — or any request at all.

---

## Where it runs

### Every node, not just the server

t[contacts.node.every-node]
The contacts backend — the directory and the CardDAV pull — is built for each
org by the boot every node runs (`build_org_state`): a hosted `task-server`, a
desktop or phone's own node (`LocalNode`), and an org a node joins after boot.
There is no server-only construction. Every caller reaches it through the
org's router behind the permission gate, so a device's own lane is a signed-in
person's like any other, and a client's call site is the same against a host
and against the device. With no server anywhere, two device nodes each list
and edit contacts, and an edit made on one is pulled by the other through the
ordinary vault sync. The example's seeded contacts are served by a device node
exactly as by a server.

### CardDAV sync with the host absent

t[contacts.sync.host-absent]
CardDAV sync runs on any node that holds the account and its secret, with the
always-on host stopped or never booted (ADR 0006 host-absent stage). The
people it pulls reach the node's peers with the vault; the account, being
node-private (`vault.private.node-only`), and its password stay on the node
that holds them.

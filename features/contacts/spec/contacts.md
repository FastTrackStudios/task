# Contacts Spec

The contacts directory (`Records/contacts/<id>.md`) and the CardDAV accounts
that fill it from Nextcloud, iCloud or any CardDAV server. This spec is about
the credential those accounts carry: an app-specific password that unlocks a
person's whole address book, and on iCloud more than that. It must reach the
CardDAV server it was issued for and nothing else — not the vault, not a peer,
not another host.

Implementation: `features/contacts/contacts` (`credentials`, `carddav`,
`vault_contacts`). The node-private directory the accounts live in is
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

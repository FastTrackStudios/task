# Test-only TLS material for the fake CardDAV server

`ca.pem` is a throwaway certificate authority; `server.pem` / `server.key` is a
leaf it signed for `localhost` and `127.0.0.1`, valid until 2126. The CA's own
private key was deleted the moment the leaf was signed, so nothing else can
ever be issued under it.

Used only by `contacts::testing::FakeCardDav` and the tests that dial it: the
client trusts `ca.pem` through `Transport::with_extra_ca_pem` (or
`TASK_CARDDAV_EXTRA_CA` on a server), never by default. The CardDAV client is
https-only, so the fake has to speak TLS for a test to reach it at all.

Regenerate (only if it ever has to change) with `openssl req -x509` for the CA
and `openssl x509 -req … -extfile` for the leaf, SAN
`DNS:localhost,IP:127.0.0.1`, `basicConstraints=CA:FALSE`,
`extendedKeyUsage=serverAuth`; then delete the CA key.

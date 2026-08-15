# Vendored vox TypeScript runtime

These four packages are copied verbatim (minus tests and a few
package.json fields) from the vox repo's `typescript/packages/` tree.
They are the runtime the generated clients in `../src/generated/`
import:

| package                 | role                                            |
| ----------------------- | ----------------------------------------------- |
| `@bearcove/vox-postcard`| postcard (wire) encode/decode + `Schema` types  |
| `@bearcove/vox-wire`    | frame codec, wire types, `RpcError` payloads    |
| `@bearcove/vox-core`    | session/handshake, `Caller`, dispatcher runtime |
| `@bearcove/vox-ws`      | browser `WebSocket` Link + `wsConnector`        |

## Pinned source

- repo: `https://codeberg.org/FastTrackStudios/vox.git`
  (workspace `Cargo.toml` aliases it as `github.com/bearcove/vox`)
- rev: `b3d806f868c7b1247564d0c0b27fd35d1b41cfd8`
  — the same rev the Rust workspace pins in `Cargo.lock`, so the
  vendored TS runtime and the server's vox protocol can't drift apart.
- copied from: `/run/media/Development/vox` (fork checkout, fix/wasm-channel-credit)

## Why copy instead of `file:`-link into the cargo checkout?

The checkout path under `~/.cargo/git/checkouts/` embeds a URL hash and
a short rev; `cargo update` swaps it out from under any symlink/file:
link without warning. A committed copy pinned to the locked rev is
boring and survives `cargo update` (you re-vendor *deliberately*, when
the workspace bumps vox).

## Local modifications (everything else is byte-identical)

Applied by `../scripts/vendor-vox.sh`:

1. `src/**/*.test.ts` deleted — they import `vitest`, which we don't
   install (devDependencies are stripped, see 2).
2. `package.json`: `scripts`, `devDependencies`, `publishConfig`, and
   `files` removed. We consume the TS source directly via the dev
   `exports: { ".": "./src/index.ts" }` entry; nothing is built or
   tested inside vendor/.
3. `vox-ws/package.json`: dependency on `@bearcove/vox-tcp` removed.
   Its runtime import graph (`src/index.ts` → `src/transport.ts`) only
   touches `@bearcove/vox-core`; the tcp package is node-only
   (`node:net`) and was only used by the deleted transport test.

## How to re-vendor (after the workspace bumps its vox rev)

```sh
# 1. find the new checkout (rev short-hash directory)
ls ~/.cargo/git/checkouts/ | grep vox
# 2. re-run the script against it
./scripts/vendor-vox.sh ~/.cargo/git/checkouts/vox-<hash>/<rev7>
# 3. regenerate the service clients against the same rev
(cd .. && cargo xtask codegen)
# 4. update the rev above, then: pnpm install && pnpm build
```

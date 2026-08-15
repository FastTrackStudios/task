# Task macOS — TestFlight for Mac

How the Task **desktop** app (`apps/task/desktop`, crate `task-app-desktop`)
ships to TestFlight for macOS, auto-updating on testers' Macs. The macOS
mirror of the iOS flow (`.github/workflows/task-ios.yml` →
`apps/fasttrackstudio/ios/deploy-testflight.sh`).

## The moving parts

| piece | where |
| --- | --- |
| CI workflow (merge to main → TestFlight) | `.github/workflows/task-macos.yml` |
| build+sign+pkg+upload script | `apps/task/desktop/macos/deploy-testflight-macos.sh` |
| one-time machine setup (signing identities) | `apps/task/desktop/macos/setup-macos.sh` |
| App Sandbox entitlements (base) | `apps/task/desktop/macos/Task.entitlements` |
| Mac App Store profile mint (ASC API) | `apps/task/desktop/macos/mint-mas-profile.rb` |
| Mac Installer Distribution cert mint (ASC API) | `apps/task/desktop/macos/mint-mac-installer.rb` |

Everything runs on **airlock** (the headless Mac mini that is already the
iOS TestFlight runner), against the same `fts-build.keychain` and the same
App Store Connect API key in `~/.appstoreconnect/`.

## Identity & versioning

- **Bundle id**: `app.fasttrackstudio.task` — the SAME app record as the
  iOS/watchOS app (registered universal in App Store Connect). The macOS
  build attaches to that record as its macOS platform; in App Store
  Connect / TestFlight it shows up under the app's **macOS** tab.
- **`CFBundleShortVersionString`** (`MARKETING_VER`, workflow input,
  default `0.0.1`): keep it in step with the iOS build's marketing version
  — both platforms share the app record and Apple groups TestFlight builds
  by version string.
- **`CFBundleVersion`** (`BUILD_NO`): unix time at build, exactly the iOS
  scheme. App Store Connect tracks build numbers **per platform**, so the
  macOS train can never collide with the iOS one; unix time keeps each
  train monotonic and doubles as "when was this uploaded".

## What the deploy script does

1. Unlock `fts-build.keychain`; look up the two signing identities
   (fails fast with a pointer to `setup-macos.sh` if either is missing).
2. Mint/refresh the **Mac App Store provisioning profile**
   (`MAC_APP_STORE` type) for the bundle id via the ASC API — re-minted
   every run, like the iOS flow.
3. `nix develop` → tailwind (`apps/task/tailwind.css`, the shared input)
   → `dx build --platform macos --release` for `task-app-desktop`.
   No `DEVELOPER_DIR` override — on the macOS-27 host, build scripts must
   link the flake apple-sdk (the iOS flow's host-SDK trap applies here too).
4. Patch `Contents/Info.plist`: product name "Task", versions, category
   (`public.app-category.productivity`), export-compliance false, and the
   `DT*` SDK-metadata keys Xcode would have stamped (App Store Connect
   rejects uploads without them — same 90534-class issue iOS hit).
5. Generate `AppIcon.icns` from the shared 1024px master
   (`apps/task/mobile/ios/…/icon-1024.png`) with `sips` + `iconutil` —
   macOS needs no actool/Assets.car, so the Xcode-beta actool dance from
   the iOS flow does not apply.
6. Entitlements = profile entitlements (`com.apple.application-identifier`,
   `com.apple.developer.team-identifier`) + the committed base
   (`Task.entitlements`). Embed the profile at
   `Contents/embedded.provisionprofile`, codesign with **Apple
   Distribution** (no hardened runtime — TestFlight/MAS wants the App
   Sandbox instead).
7. `productbuild --component … --sign "Mac Installer Distribution"` →
   signed installer `.pkg` (the artifact macOS uploads; iOS uploads an
   `.ipa`).
8. `xcrun altool --upload-app -t macos` with the ASC API key. `DRY_RUN=1`
   stops right before this step, leaving the verified `.pkg` on disk.

## Entitlements (and why)

- `com.apple.security.app-sandbox` — **mandatory** for TestFlight/MAS.
- `com.apple.security.network.client` — the app is a vox client
  (WebSocket/HTTP to task-server); outbound only, no server embedded.
- `com.apple.security.files.user-selected.read-write` — powerbox open/save
  panels (WKWebView uploads/downloads). The app's own state (auth tokens,
  server registry under `$XDG_DATA_HOME/task/`) lands inside the sandbox
  container and needs no entitlement.
- **Known limit**: if the desktop app ever grows "point me at a local vault
  folder and watch it" behavior, user-selected covers the initial pick but
  persistent across-launch access will need security-scoped bookmarks
  (`com.apple.security.files.bookmarks.app-scope`) + code to redeem them.
  WKWebView itself is sandbox-clean (its web/network processes are Apple's).

## One-time setup (already scripted)

On airlock, once:

```bash
ssh rat@airlock.local
export PATH="/nix/var/nix/profiles/default/bin:$PATH"
cd <checkout> && bash apps/task/desktop/macos/setup-macos.sh
```

It creates/unlocks the build keychain and ensures both identities exist,
minting anything missing via the ASC API (`Apple Distribution` was already
there from the iOS flow; `Mac Installer Distribution` gets created +
imported). No Xcode UI, no Developer-portal clicking.

## Dry-run status (2026-07-28)

Validated on airlock, end-to-end except the two deliberately-skipped steps:

- `setup-macos.sh` ran for real: the **Mac Installer Distribution cert was
  created via the ASC API** (`certificateType: MAC_INSTALLER_DISTRIBUTION`
  — the enum works) and imported; identity
  `3rd Party Mac Developer Installer: CODY JAMES WRIGHT (28C2G63DA7)`.
- `mint-mas-profile.rb` minted a `MAC_APP_STORE` profile against the
  universal bundle id — no portal registration needed.
- The full plist-patch → icns → entitlements-merge → profile-embed →
  codesign → `productbuild` chain ran and verified (`codesign --verify
  --deep --strict` clean; sealed entitlements carry app-sandbox +
  network.client + user-selected + the profile's identifiers; `pkgutil
  --check-signature` shows the full Apple chain). A handful of
  `write: Permission denied` lines from productbuild are cosmetic — the
  signed pkg is produced and verifies.
- The dx macOS release build itself is proven on this box (it produced a
  real TaskAppDesktop.app on Jul 18) but was NOT re-run to completion in
  the dry run: airlock's disk hit 100% while nix rebuilt the devshell
  (flake.lock drift forces a from-source dioxus-cli build). Free space
  first — see below.
- No upload was performed (deliberate).

## Manual steps that remain (user)

1. **Free disk on airlock** (chronically ~full; the dry run died at
   ~0.6 GB free). Safe reclaims per the airlock-ios skill: the CI
   runner's `target/` (~4 GB, cold-rebuilds itself) and
   `~/fts/target/wasm32-unknown-unknown` (~1.7 GB). The first real build
   wants ≥10 GB headroom for the devshell rebuild + release target churn.
2. **First upload**: run the workflow (or the script without `DRY_RUN=1`)
   once, deliberately — CI is wired, but the first real upload to the app
   record is a human call.
3. **App Store Connect, after the first macOS build processes**: under the
   app → TestFlight → **macOS** → add the build to the internal tester
   group (internal testers usually inherit; verify once). Export-compliance
   is answered by the plist key, so normally no questionnaire appears.
4. **Nothing else**: certs and the provisioning profile are API-minted;
   the universal bundle id already exists.

## Installing on your laptop + auto-update

1. On the Mac, install **TestFlight** (Mac App Store; macOS 12+).
2. Sign in with the same Apple ID that's an internal tester on the
   FastTrackStudio team (or redeem the invite link once).
3. Task appears under the app's macOS builds → **Install**. TestFlight
   for Mac keeps it updated: with "Automatic Updates" on (default), every
   merge to main that ships a build lands on the laptop within
   TestFlight's polling window (typically same-day, no user action);
   builds expire after 90 days like iOS.

## Iterating / debugging

```bash
# full dry run on airlock (no upload):
KEYCHAIN=fts-build.keychain KEYCHAIN_PW=fts-build DRY_RUN=1 \
  bash apps/task/desktop/macos/deploy-testflight-macos.sh

# reuse the built .app while iterating on sign/pkg:
SKIP_BUILD=1 DRY_RUN=1 KEYCHAIN=… bash …/deploy-testflight-macos.sh
```

- Build log: `/tmp/task-macos-build.log` (dx exit codes lie; the script
  gates on the `.app` existing).
- The signed `.pkg` path is printed at the end; `pkgutil
  --check-signature` output is echoed for sanity.
- Repo gate: `cargo check -p task-app-desktop` must stay clean; the
  scripts are `bash -n`/`ruby -c` clean.

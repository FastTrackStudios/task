#!/usr/bin/env bash
# The Info.plist keys the Task app needs on iOS, written into a dx-built
# .app — sourced by build-ios.sh (simulator / dev) and deploy-testflight.sh,
# so the two builds cannot drift apart (the shape Signal's
# apps/desktop/ios/app-plist.sh set).
#
#   source ios/app-plist.sh && task_app_plist "$APP"
#
# Release-only keys (versions, SDK metadata, export compliance) stay in
# deploy-testflight.sh; everything that changes how the app RUNS is here.

task_app_plist() {
    local plist="$1/Info.plist"
    local pb=/usr/libexec/PlistBuddy
    local bundle
    bundle="$($pb -c 'Print :CFBundleIdentifier' "$plist")"

    # The UIScene lifecycle — iOS 27 stops an app built with its SDK at
    # launch without it (`_UIApplicationEvaluateRuntimeIssueForNoScene-
    # LifecycleAdoption` traps). Unlike Signal's winit, tao (the window
    # under Dioxus's webview) adopts scenes itself: with
    # UIApplicationSupportsMultipleScenes set, its app delegate answers
    # `configurationForConnectingSceneSession` with its own
    # TaoSceneDelegate and puts the window in that scene. So the manifest
    # names no delegate class — tao's configuration supplies it.
    $pb -c "Delete :UIApplicationSceneManifest" "$plist" 2>/dev/null || true
    $pb \
        -c "Add :UIApplicationSceneManifest dict" \
        -c "Add :UIApplicationSceneManifest:UIApplicationSupportsMultipleScenes bool true" \
        "$plist"

    # App bundle OS type — App Store requires CFBundlePackageType=APPL (dx omits it).
    $pb -c "Set :CFBundlePackageType APPL" "$plist" 2>/dev/null \
        || $pb -c "Add :CFBundlePackageType string APPL" "$plist"

    # Launch screen: the system one, on the app's own background colour
    # (LaunchBackground in Assets.xcassets, when the catalog is compiled
    # in), so opening the app is not a white flash before a dark shell.
    $pb -c "Delete :UILaunchStoryboardName" "$plist" 2>/dev/null || true
    $pb -c "Delete :UILaunchScreen" "$plist" 2>/dev/null || true
    $pb -c "Add :UILaunchScreen dict" "$plist"
    $pb -c "Add :UILaunchScreen:UIColorName string LaunchBackground" "$plist"

    # ProMotion: an iPhone holds an app to 60 Hz without this, and the
    # webview's scrolling and sheet animations read as laggy next to the
    # system's own.
    $pb -c "Delete :CADisableMinimumFrameDurationOnPhone" "$plist" 2>/dev/null || true
    $pb -c "Add :CADisableMinimumFrameDurationOnPhone bool true" "$plist"

    # A phone app is upright: the layout is a portrait one (bottom tab
    # bar, one column). An iPad keeps every orientation, so it keeps
    # multitasking — App Store Connect refuses a bundle that limits an
    # iPad's orientations without giving multitasking up (90474).
    orientations() {
        local key="$1"; shift
        $pb -c "Delete :$key" "$plist" 2>/dev/null || true
        $pb -c "Add :$key array" "$plist"
        local i=0
        for o in "$@"; do
            $pb -c "Add :$key:$i string UIInterfaceOrientation$o" "$plist"
            i=$((i + 1))
        done
    }
    orientations "UISupportedInterfaceOrientations~iphone" Portrait
    orientations "UISupportedInterfaceOrientations~ipad" Portrait PortraitUpsideDown LandscapeLeft LandscapeRight
    orientations UISupportedInterfaceOrientations Portrait PortraitUpsideDown LandscapeLeft LandscapeRight

    # URL scheme = bundle id. The central sign-in (`ui::central_login::native`,
    # `ios_auth.rs`) sends the person to the issuer in an
    # ASWebAuthenticationSession and gets the redirect back on
    # `<bundle id>://auth/callback`; the issuer has exactly that URI
    # registered for this app (fts-auth.nix). Declaring the scheme here is
    # what lets the system hand the URL to this app and no other.
    $pb -c "Delete :CFBundleURLTypes" "$plist" 2>/dev/null || true
    $pb -c "Add :CFBundleURLTypes array" "$plist"
    $pb -c "Add :CFBundleURLTypes:0 dict" "$plist"
    $pb -c "Add :CFBundleURLTypes:0:CFBundleURLName string $bundle" "$plist"
    $pb -c "Add :CFBundleURLTypes:0:CFBundleTypeRole string Editor" "$plist"
    $pb -c "Add :CFBundleURLTypes:0:CFBundleURLSchemes array" "$plist"
    $pb -c "Add :CFBundleURLTypes:0:CFBundleURLSchemes:0 string $bundle" "$plist"

    # The app's name. dx derives the bundle name from the Dioxus.toml
    # `[application] name`, so `task-app-mobile` showed as "TaskAppMobile".
    # CFBundleDisplayName is the home screen's; CFBundleName is what the
    # system's own prompts say — the sign-in sheet's '"TaskAppMobile"
    # Wants to Use "auth.fasttrackstudio.app"' — so both are set.
    local name="${APP_DISPLAY_NAME:-Task}"
    $pb -c "Set :CFBundleDisplayName $name" "$plist" 2>/dev/null \
        || $pb -c "Add :CFBundleDisplayName string $name" "$plist"
    $pb -c "Set :CFBundleName $name" "$plist" 2>/dev/null \
        || $pb -c "Add :CFBundleName string $name" "$plist"

    # Minimum OS — App Store rejects a bundle without it (dx doesn't emit one).
    $pb -c "Set :MinimumOSVersion 15.0" "$plist" 2>/dev/null \
        || $pb -c "Add :MinimumOSVersion string 15.0" "$plist"

    # Local network: an org can be reached over iroh's direct paths on the
    # same Wi-Fi; without this key iOS silently drops that traffic.
    $pb -c "Delete :NSLocalNetworkUsageDescription" "$plist" 2>/dev/null || true
    $pb -c "Add :NSLocalNetworkUsageDescription string 'Connects to your studio on the local network so your work syncs directly.'" "$plist"
    # The Bonjour type that RAISES the local-network prompt — iroh's raw
    # UDP gets silently filtered instead of prompting without it.
    $pb -c "Delete :NSBonjourServices" "$plist" 2>/dev/null || true
    $pb -c "Add :NSBonjourServices array" "$plist"
    $pb -c "Add :NSBonjourServices:0 string _fts._tcp" "$plist"
    # Documents shows in the Files app.
    $pb -c "Add :UIFileSharingEnabled bool true" "$plist" 2>/dev/null || true
    $pb -c "Add :LSSupportsOpeningDocumentsInPlace bool true" "$plist" 2>/dev/null || true
}

use dioxus::prelude::*;
use ui::App;

const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");
const MANIFEST: Asset = asset!("/assets/manifest.json");
const SERVICE_WORKER: Asset = asset!("/assets/sw.js");

// PWA icons. The manifest references these by their literal
// `/assets/icon-*.png` paths (a static JSON file can't know the
// fingerprint hash), so they are bundled with the hash suffix
// disabled — the bundled name must match the manifest `src` exactly.
const ICON_192: Asset = asset!(
    "/assets/icon-192.png",
    AssetOptions::image().with_hash_suffix(false)
);
const ICON_512: Asset = asset!(
    "/assets/icon-512.png",
    AssetOptions::image().with_hash_suffix(false)
);
const ICON_MASKABLE_512: Asset = asset!(
    "/assets/icon-maskable-512.png",
    AssetOptions::image().with_hash_suffix(false)
);
const APPLE_TOUCH_ICON: Asset = asset!(
    "/assets/apple-touch-icon.png",
    AssetOptions::image().with_hash_suffix(false)
);

fn main() {
    // Quiet the browser console: vox-core emits a flood of per-request
    // DEBUG tracing on wasm. Cap at INFO so our own info/warn/error and
    // genuine problems still surface, but the request-loop chatter
    // doesn't drown them.
    let _ = dioxus::logger::init(dioxus::logger::tracing::Level::INFO);
    dioxus::launch(Root);
}

#[component]
fn Root() -> Element {
    // Register the service worker once at app boot. The asset!()
    // macro fingerprints the URL so the SW path includes a hash;
    // we pass it through `document::eval` rather than a literal
    // string in the JS so each rebuild registers the right file.
    //
    // Scope note: the SW lives under `/assets/` so its default
    // scope is `/assets/` — that's enough to cache the CSS, JS
    // chunks, and wasm bundle (the bulk of the offline payload).
    // To widen scope to `/` (so the SPA root and arbitrary routes
    // also serve from cache), the server must send the
    // `Service-Worker-Allowed: /` HTTP header on the SW response.
    // Most prod deploys are one-line nginx/caddy snippets; the
    // dev server may not support it, in which case PWA install
    // still works but offline-page-load is limited to the asset
    // dir.
    use_hook(|| {
        let sw_url = SERVICE_WORKER.to_string();
        spawn(async move {
            let js = format!(
                r"if ('serviceWorker' in navigator) {{
                    navigator.serviceWorker.register('{sw_url}')
                        .then(r => console.log('sw registered', r.scope))
                        .catch(e => console.warn('sw register failed', e));
                }}"
            );
            let _ = document::eval(&js).await;
        });
    });
    rsx! {
        document::Stylesheet { href: TAILWIND_CSS }
        document::Link {
            rel: "manifest",
            href: MANIFEST,
        }
        // Icon links double as the keep-alive references that make the
        // bundler include the PNGs the manifest points at.
        document::Link {
            rel: "icon",
            r#type: "image/png",
            sizes: "192x192",
            href: ICON_192,
        }
        document::Link {
            rel: "icon",
            r#type: "image/png",
            sizes: "512x512",
            href: ICON_512,
        }
        document::Link {
            rel: "icon",
            r#type: "image/png",
            sizes: "512x512",
            href: ICON_MASKABLE_512,
        }
        document::Link {
            rel: "apple-touch-icon",
            sizes: "180x180",
            href: APPLE_TOUCH_ICON,
        }
        document::Meta {
            name: "theme-color",
            content: "#1e1e1e",
        }
        document::Meta {
            name: "viewport",
            content: "width=device-width, initial-scale=1, viewport-fit=cover",
        }
        document::Meta {
            name: "mobile-web-app-capable",
            content: "yes",
        }
        // Deprecated in favor of mobile-web-app-capable, but older iOS
        // Safari only honors the apple- prefixed variant.
        document::Meta {
            name: "apple-mobile-web-app-capable",
            content: "yes",
        }
        document::Meta {
            name: "apple-mobile-web-app-status-bar-style",
            content: "black-translucent",
        }
        App {}
    }
}

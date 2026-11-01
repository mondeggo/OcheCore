//! The browser UI, compiled into the binary so the daemon serves it on its own
//! port with no files to install and nothing downloaded at runtime.
use axum::{
    http::{StatusCode, header},
    response::{IntoResponse, Redirect, Response},
};

struct Asset {
    path: &'static str,
    content_type: &'static str,
    body: &'static [u8],
}

macro_rules! assets {
    ($($path:literal => $content_type:expr),* $(,)?) => {
        &[$(Asset {
            path: $path,
            content_type: $content_type,
            body: include_bytes!(concat!("../ui/", $path)),
        }),*]
    };
}

const HTML: &str = "text/html; charset=utf-8";

const ASSETS: &[Asset] = assets![
    "index.html" => HTML,
    "setup/index.html" => HTML,
    "setup/cameras.html" => HTML,
    "setup/calibrate.html" => HTML,
    "setup/test.html" => HTML,
    "debug/index.html" => HTML,
    "debug/debug.js" => "text/javascript; charset=utf-8",
    "assets/app.js" => "text/javascript; charset=utf-8",
    "assets/style.css" => "text/css; charset=utf-8",
    "assets/icon.svg" => "image/svg+xml",
];

/// Pages load only their own scripts, styles and camera snapshots.
const POLICY: &str = "default-src 'self'; img-src 'self' blob:; object-src 'none'; \
                      base-uri 'none'; form-action 'self'; frame-ancestors 'none'";

fn find(path: &str) -> Option<&'static Asset> {
    ASSETS.iter().find(|asset| asset.path == path)
}

/// The UI file for a request path. Directories serve their `index.html`; a
/// directory named without its trailing slash redirects, so the pages'
/// relative links resolve.
pub(crate) fn response(path: &str) -> Option<Response> {
    let relative = path.strip_prefix('/')?;
    let asset = if relative.is_empty() || relative.ends_with('/') {
        find(&format!("{relative}index.html"))?
    } else if let Some(asset) = find(relative) {
        asset
    } else {
        find(&format!("{relative}/index.html"))?;
        return Some(Redirect::permanent(&format!("{path}/")).into_response());
    };
    Some(
        (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, asset.content_type),
                // Revalidate so an upgraded daemon never serves a stale page.
                (header::CACHE_CONTROL, "no-cache"),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
                (header::REFERRER_POLICY, "no-referrer"),
                (header::CONTENT_SECURITY_POLICY, POLICY),
            ],
            asset.body,
        )
            .into_response(),
    )
}

//! Static SPA serving. With `--assets-dir` we serve a built `web/dist` from disk (SPA fallback to
//! index.html). Otherwise, when built with the `embed-ui` feature the SPA is served from bytes
//! baked into the binary; without it, a tiny placeholder page describes the API.

use std::path::PathBuf;

use axum::response::Html;
use axum::Router;
use tower_http::services::{ServeDir, ServeFile};

const PLACEHOLDER: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>mgmt</title>
<meta name="viewport" content="width=device-width, initial-scale=1">
<style>body{font:16px system-ui,sans-serif;max-width:40rem;margin:3rem auto;padding:0 1rem;color:#cdd6f4;background:#1e1e2e}
code{background:#313244;padding:.1rem .3rem;border-radius:.2rem}a{color:#89b4fa}</style></head>
<body><h1>mgmt web</h1><p>The API is up. The PWA is not built into this binary.</p>
<p>Try <code>GET /api/board</code>, <code>/api/tasks</code>, <code>/api/events?from=&amp;to=</code>,
<code>/api/meta</code>, or <code>/api/status</code>.</p>
<p>Run with <code>--assets-dir web/dist</code> (or build with the <code>embed-ui</code> feature) to serve the app.</p>
</body></html>"#;

/// Attach static serving as the router's fallback (everything not under `/api`). A `--assets-dir`
/// takes precedence (serve from disk); otherwise the embedded SPA (feature `embed-ui`) or the
/// placeholder.
pub fn attach(app: Router, assets_dir: Option<PathBuf>) -> Router {
    if let Some(dir) = assets_dir {
        let index = dir.join("index.html");
        let serve = ServeDir::new(dir).fallback(ServeFile::new(index));
        return app.fallback_service(serve);
    }
    embedded_or_placeholder(app)
}

#[cfg(not(feature = "embed-ui"))]
fn embedded_or_placeholder(app: Router) -> Router {
    app.fallback(placeholder)
}

#[cfg(feature = "embed-ui")]
fn embedded_or_placeholder(app: Router) -> Router {
    app.fallback(embedded::serve)
}

async fn placeholder() -> Html<&'static str> {
    Html(PLACEHOLDER)
}

/// The SPA baked into the binary at build time from `web/dist`.
#[cfg(feature = "embed-ui")]
mod embedded {
    use axum::body::Body;
    use axum::extract::Request;
    use axum::http::{header, StatusCode, Uri};
    use axum::response::{IntoResponse, Response};
    use rust_embed::RustEmbed;

    #[derive(RustEmbed)]
    #[folder = "../../web/dist"]
    struct Dist;

    /// Serve an embedded asset by path, falling back to `index.html` for SPA client routes.
    pub async fn serve(req: Request) -> Response {
        let path = req.uri().path().trim_start_matches('/');
        let path = if path.is_empty() { "index.html" } else { path };
        match Dist::get(path).or_else(|| Dist::get("index.html")) {
            Some(file) => {
                let mime = mime_for(req.uri());
                ([(header::CONTENT_TYPE, mime)], Body::from(file.data.into_owned())).into_response()
            }
            None => (StatusCode::NOT_FOUND, "not found").into_response(),
        }
    }

    fn mime_for(uri: &Uri) -> &'static str {
        let p = uri.path();
        if p.ends_with(".js") {
            "text/javascript"
        } else if p.ends_with(".css") {
            "text/css"
        } else if p.ends_with(".png") {
            "image/png"
        } else if p.ends_with(".svg") {
            "image/svg+xml"
        } else if p.ends_with(".webmanifest") {
            "application/manifest+json"
        } else {
            "text/html; charset=utf-8"
        }
    }
}

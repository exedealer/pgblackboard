use super::AppError;
use axum::http::Uri;
use axum::response::Response;

/*

TODO treat index.html as dynamic template, not static
  .route("/", serve_page("./ui/index.html"))
  .nest("/static", serve_static_from_fs("./ui/static/"))
  .fallback(serve_page("./ui/error.html", 404))

- ui
  - index.html
  - error.html
  - static
  - main.js
  - style.css
  - favicon.svg
 */

// TODO decompress if not accept: gzip
// TODO Etag or other caching
#[cfg(not(debug_assertions))]
pub async fn serve_ui(uri: Uri) -> Result<Response, AppError> {
  match uri.path() {
    "/" => Response::builder()
      .header("content-type", "text/html; charset=utf-8")
      .body(include_bytes!("../ui/.bundle/index.html").as_ref().into()),

    "/favicon.svg" => Response::builder()
      .header("content-type", "image/svg+xml; charset=utf-8")
      .body(include_bytes!("../ui/.bundle/favicon.svg").as_ref().into()),

    "/main.js" => Response::builder()
      .header("content-type", "text/javascript; charset=utf-8")
      .header("content-encoding", "gzip")
      .body(include_bytes!("../ui/.bundle/main.js.gz").as_ref().into()),

    "/map.js" => Response::builder()
      .header("content-type", "text/javascript; charset=utf-8")
      .header("content-encoding", "gzip")
      .body(include_bytes!("../ui/.bundle/map.js.gz").as_ref().into()),

    "/style.css" => Response::builder()
      .header("content-type", "text/css; charset=utf-8")
      .header("content-encoding", "gzip")
      .body(include_bytes!("../ui/.bundle/style.css.gz").as_ref().into()),

    _ => Response::builder()
      .status(axum::http::StatusCode::NOT_FOUND)
      .header("content-type", "text/plain; charset=utf-8")
      .body("not found\n".into()),
  }
  .map_err(|err| err.into())
}

#[cfg(debug_assertions)]
pub async fn serve_ui(uri: Uri) -> Result<Response, AppError> {
  // avoid conditional tokio/fs feature assache.
  // we using blocking fs only for development
  use std::fs;
  use std::io;

  let path = match uri.path() {
    // TODO "/index.html" => "not_found"
    "/" => "/index.html",
    p => p,
  };

  // TODO fix parent traverse
  let file_path = ["./ui", path].concat();
  let file_body = match fs::read(file_path) {
    Ok(body) => body,
    Err(err) => {
      return match err.kind() {
        io::ErrorKind::NotFound | io::ErrorKind::IsADirectory => {
          Response::builder()
            .status(axum::http::StatusCode::NOT_FOUND)
            .header("content-type", "text/plain; charset=utf-8")
            .body("not found\n".into())
            .map_err(|err| err.into())
        }
        _ => Err(err.into()),
      };
    }
  };

  // TODO case insensitive
  let content_type = match path {
    p if p.ends_with(".html") => "text/html; charset=utf-8",
    p if p.ends_with(".css") => "text/css; charset=utf-8",
    p if p.ends_with(".js") => "text/javascript; charset=utf-8",
    p if p.ends_with(".svg") => "image/svg+xml; charset=utf-8",
    p if p.ends_with(".woff2") => "font/woff2",
    _ => "application/octet-stream",
  };

  Response::builder()
    .header("content-type", content_type)
    .body(file_body.into())
    .map_err(|err| err.into())
}

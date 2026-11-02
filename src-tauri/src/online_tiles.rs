//! Range-request proxy for the online map: `http://mo-tiles.localhost/<build>.pmtiles` forwards
//! the webview's byte ranges to `build.protomaps.com` and answers with CORS headers, so the
//! pmtiles protocol can read the planet-wide build directly when no local map is installed.
use std::sync::OnceLock;

use tauri::http::{header, Request, Response, StatusCode};

use crate::data::tiles::BUILD_BASE;

fn client() -> &'static reqwest::blocking::Client {
    static C: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    C.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .user_agent(crate::data::download::USER_AGENT)
            .connect_timeout(std::time::Duration::from_secs(15))
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .expect("http client")
    })
}

fn valid_key(name: &str) -> bool {
    name.len() == 16 && name.ends_with(".pmtiles") && name[..8].bytes().all(|b| b.is_ascii_digit())
}

fn simple(status: StatusCode, msg: &str) -> Response<Vec<u8>> {
    Response::builder().status(status).header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*").body(msg.as_bytes().to_vec()).unwrap()
}

pub fn proxy(request: Request<Vec<u8>>) -> Response<Vec<u8>> {
    let path = request.uri().path().trim_start_matches('/').to_string();
    if request.method() == tauri::http::Method::OPTIONS {
        return Response::builder()
            .status(StatusCode::NO_CONTENT)
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .header(header::ACCESS_CONTROL_ALLOW_HEADERS, "range")
            .header(header::ACCESS_CONTROL_ALLOW_METHODS, "GET, HEAD, OPTIONS")
            .body(Vec::new())
            .unwrap();
    }
    if !valid_key(&path) {
        return simple(StatusCode::NOT_FOUND, "unknown build");
    }
    let url = format!("{BUILD_BASE}{path}");
    let mut req = client().get(&url);
    if let Some(r) = request.headers().get(header::RANGE).and_then(|v| v.to_str().ok()) {
        req = req.header(header::RANGE, r);
    }
    match req.send() {
        Ok(up) => {
            let status = up.status();
            let mut b = Response::builder()
                .status(status.as_u16())
                .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
                .header(header::ACCESS_CONTROL_EXPOSE_HEADERS, "content-range, content-length, accept-ranges, etag")
                .header(header::ACCEPT_RANGES, "bytes")
                .header(header::CONTENT_TYPE, "application/octet-stream");
            for h in [header::CONTENT_RANGE, header::CONTENT_LENGTH, header::ETAG] {
                if let Some(v) = up.headers().get(&h) {
                    b = b.header(&h, v.clone());
                }
            }
            match up.bytes() {
                Ok(body) => b.body(body.to_vec()).unwrap(),
                Err(e) => simple(StatusCode::BAD_GATEWAY, &e.to_string()),
            }
        }
        Err(e) => simple(StatusCode::BAD_GATEWAY, &e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unknown_paths() {
        let req = Request::builder().uri("http://mo-tiles.localhost/../etc/passwd").body(Vec::new()).unwrap();
        assert_eq!(proxy(req).status(), StatusCode::NOT_FOUND);
        assert!(valid_key("20261102.pmtiles") && !valid_key("2026110.pmtiles") && !valid_key("20261102.json"));
    }

    /// Needs the network: a 128-byte range of today's build comes back as a 206 with CORS.
    #[test]
    #[ignore]
    fn proxies_a_range() {
        let key = crate::data::tiles::latest_build_key(&crate::data::download::http_client().unwrap()).unwrap();
        let req = Request::builder().uri(format!("http://mo-tiles.localhost/{key}")).header(header::RANGE, "bytes=0-127").body(Vec::new()).unwrap();
        let r = proxy(req);
        assert_eq!(r.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(r.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN).unwrap(), "*");
        assert_eq!(r.body().len(), 128);
        assert_eq!(&r.body()[..7], b"PMTiles");
    }
}

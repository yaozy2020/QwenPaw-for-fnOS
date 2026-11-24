use super::unique_header;
use axum::{
    body::Body,
    http::{header, HeaderMap, HeaderValue, Request, StatusCode},
    response::Response,
};

fn browser_scheme(headers: &HeaderMap) -> Result<Option<super::OriginScheme>, ()> {
    if matches!(
        unique_header(headers, "sec-fetch-site")?,
        Some("cross-site" | "same-site")
    ) {
        return Err(());
    }
    let Some(origin) = unique_header(headers, "origin")? else {
        return Ok(None);
    };
    let browser = super::parse_origin(origin).ok_or(())?;
    let host = unique_header(headers, "host")?.ok_or(())?;
    let external = super::parse_authority(browser.scheme, host).ok_or(())?;
    (browser == external)
        .then_some(Some(browser.scheme))
        .ok_or(())
}

fn stateless_api_write(headers: &HeaderMap) -> bool {
    if headers.contains_key(header::COOKIE) || headers.contains_key("sec-fetch-site") {
        return false;
    }
    let json = unique_header(headers, "content-type").is_ok_and(|value| {
        value.is_some_and(|value| {
            value
                .split(';')
                .next()
                .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
        })
    });
    let bearer = unique_header(headers, "authorization").is_ok_and(|value| {
        value.is_some_and(|value| value.len() > 7 && value[..7].eq_ignore_ascii_case("bearer "))
    });
    json || bearer
}

pub(super) fn same_origin_write(headers: &HeaderMap, public_access: bool) -> bool {
    match browser_scheme(headers) {
        Ok(Some(_)) => true,
        Err(()) => false,
        Ok(None) => !public_access || stateless_api_write(headers),
    }
}

pub(super) async fn cache_policy(request: Request<Body>, next: axum::middleware::Next) -> Response {
    let navigation = request.method() == axum::http::Method::GET;
    let hashed_asset_read = matches!(
        *request.method(),
        axum::http::Method::GET | axum::http::Method::HEAD
    ) && super::web_proxy::is_hashed_asset(request.uri().path());
    let mut response = next.run(request).await;
    if !(hashed_asset_read && response.status() == StatusCode::OK) {
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("private, no-store"),
        );
    }
    if navigation && response.status() == StatusCode::UNAUTHORIZED {
        response.headers_mut().insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(
                "default-src 'none'; script-src 'self'; style-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'",
            ),
        );
        response.headers_mut().insert(
            header::REFERRER_POLICY,
            HeaderValue::from_static("same-origin"),
        );
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt;

    fn router(status: StatusCode) -> axum::Router {
        axum::Router::new()
            .fallback(move || async move {
                Response::builder()
                    .status(status)
                    .header(header::CACHE_CONTROL, "public, max-age=31536000, immutable")
                    .body(Body::empty())
                    .unwrap()
            })
            .layer(axum::middleware::from_fn(cache_policy))
    }

    #[tokio::test]
    async fn only_successful_hashed_asset_reads_keep_immutable_cache() {
        for method in [axum::http::Method::GET, axum::http::Method::HEAD] {
            let response = router(StatusCode::OK)
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri("/assets/index-opcB8AGu.js")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                response.headers()[header::CACHE_CONTROL],
                "public, max-age=31536000, immutable"
            );
        }

        for (method, path, status) in [
            (
                axum::http::Method::POST,
                "/assets/index-opcB8AGu.js",
                StatusCode::OK,
            ),
            (axum::http::Method::GET, "/api/auth/login", StatusCode::OK),
            (
                axum::http::Method::GET,
                "/assets/index-opcB8AGu.js",
                StatusCode::NOT_FOUND,
            ),
        ] {
            let response = router(status)
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                response.headers()[header::CACHE_CONTROL],
                "private, no-store"
            );
        }
    }

    #[test]
    fn public_origin_less_writes_allow_only_stateless_api_clients() {
        let headers = HeaderMap::new();
        assert!(same_origin_write(&headers, false));
        assert!(!same_origin_write(&headers, true));

        let mut json = HeaderMap::new();
        json.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        assert!(same_origin_write(&json, true));
        json.insert(
            header::COOKIE,
            HeaderValue::from_static("qwenpaw_session=session"),
        );
        assert!(!same_origin_write(&json, true));

        let mut bearer = HeaderMap::new();
        bearer.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer token"),
        );
        assert!(same_origin_write(&bearer, true));
        bearer.insert("sec-fetch-site", HeaderValue::from_static("same-origin"));
        assert!(!same_origin_write(&bearer, true));

        let mut browser = HeaderMap::new();
        browser.insert(header::HOST, HeaderValue::from_static("nas.example:19091"));
        browser.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://nas.example:19091"),
        );
        browser.insert(
            header::COOKIE,
            HeaderValue::from_static("qwenpaw_session=session"),
        );
        assert!(same_origin_write(&browser, true));
        browser.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://attacker.example"),
        );
        assert!(!same_origin_write(&browser, true));
    }
}

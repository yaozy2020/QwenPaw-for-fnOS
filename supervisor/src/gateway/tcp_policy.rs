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

pub(super) fn same_origin_write(headers: &HeaderMap) -> bool {
    browser_scheme(headers).is_ok()
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
}

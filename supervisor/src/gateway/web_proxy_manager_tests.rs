use std::{io, net::TcpListener as StdTcpListener, sync::Arc, time::Duration};

use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
    Router,
};
use serde_json::{json, Value};
use tower::ServiceExt;

use super::WebProxyManager;
use crate::api::{
    self,
    test_support::{free_port, TestDir},
    AppState,
};

fn occupied_listener() -> (StdTcpListener, u16) {
    let listener = StdTcpListener::bind(("0.0.0.0", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    (listener, port)
}

fn persisted_port(state: &AppState) -> String {
    std::fs::read_to_string(std::path::Path::new(&state.core.var_dir).join("web_proxy_port"))
        .unwrap()
}

fn persisted_public(state: &AppState) -> String {
    std::fs::read_to_string(std::path::Path::new(&state.core.var_dir).join("web_proxy_public"))
        .unwrap()
}

async fn accepts_connections_at(host: &str, port: u16) -> bool {
    matches!(
        tokio::time::timeout(
            Duration::from_secs(2),
            tokio::net::TcpStream::connect((host, port)),
        )
        .await,
        Ok(Ok(_))
    )
}

async fn accepts_connections(port: u16) -> bool {
    accepts_connections_at("127.0.0.1", port).await
}

async fn wait_until_bindable(port: u16) -> bool {
    for _ in 0..100 {
        match StdTcpListener::bind(("0.0.0.0", port)) {
            Ok(listener) => {
                drop(listener);
                return true;
            }
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(_) => return false,
        }
    }
    false
}

#[tokio::test]
async fn manager_start_auto_assigns_and_persists_a_loopback_port() {
    let dir = TestDir::new("web-proxy-manager-auto");
    let state = dir.state(free_port());

    let manager = WebProxyManager::start(state.clone(), 0, false)
        .await
        .unwrap();
    let (port, public) = manager.state().await;

    assert_ne!(port, 0);
    assert!(!public);
    assert_eq!(persisted_port(&state), format!("{port}\n"));
    assert_eq!(persisted_public(&state), "0\n");
    assert!(accepts_connections(port).await);
    assert!(!accepts_connections_at("127.0.0.2", port).await);
}

#[tokio::test]
async fn manager_start_public_binds_all_interfaces_and_persists_public() {
    let dir = TestDir::new("web-proxy-manager-public");
    let state = dir.state(free_port());

    let manager = WebProxyManager::start(state.clone(), 0, true)
        .await
        .unwrap();
    let (port, public) = manager.state().await;

    assert_ne!(port, 0);
    assert!(public);
    assert_eq!(persisted_port(&state), format!("{port}\n"));
    assert_eq!(persisted_public(&state), "1\n");
    assert!(accepts_connections(port).await);
    assert!(accepts_connections_at("127.0.0.2", port).await);
}

#[tokio::test]
async fn manager_start_falls_back_when_the_requested_port_is_occupied() {
    let dir = TestDir::new("web-proxy-manager-fallback");
    let state = dir.state(free_port());
    let (_occupied, requested_port) = occupied_listener();

    let manager = WebProxyManager::start(state.clone(), requested_port, false)
        .await
        .unwrap();
    let (actual_port, public) = manager.state().await;

    assert_ne!(actual_port, requested_port);
    assert_ne!(actual_port, 0);
    assert!(!public);
    assert_eq!(persisted_port(&state), format!("{actual_port}\n"));
    assert_eq!(persisted_public(&state), "0\n");
    assert!(accepts_connections(actual_port).await);
}

#[tokio::test]
async fn manager_reconfigure_switches_access_after_binding_and_persisting() {
    let dir = TestDir::new("web-proxy-manager-reconfigure");
    let state = dir.state(free_port());
    let manager = WebProxyManager::start(state.clone(), 0, false)
        .await
        .unwrap();
    let (previous_port, _) = manager.state().await;

    let public_port = manager.reconfigure(previous_port, true).await.unwrap();

    assert_eq!(public_port, previous_port);
    assert_eq!(manager.state().await, (public_port, true));
    assert_eq!(persisted_port(&state), format!("{public_port}\n"));
    assert_eq!(persisted_public(&state), "1\n");
    assert!(accepts_connections(public_port).await);
    assert!(accepts_connections_at("127.0.0.2", public_port).await);

    let private_port = manager.reconfigure(public_port, false).await.unwrap();

    assert_eq!(private_port, public_port);
    assert_eq!(manager.state().await, (private_port, false));
    assert_eq!(persisted_port(&state), format!("{private_port}\n"));
    assert_eq!(persisted_public(&state), "0\n");
    assert!(accepts_connections(private_port).await);
    assert!(!accepts_connections_at("127.0.0.2", private_port).await);
}

#[tokio::test]
async fn manager_reconfigure_accepts_a_specific_free_port() {
    let dir = TestDir::new("web-proxy-manager-specific-port");
    let state = dir.state(free_port());
    let manager = WebProxyManager::start(state.clone(), 0, false)
        .await
        .unwrap();
    let (previous_port, _) = manager.state().await;
    let (reservation, requested_port) = occupied_listener();
    drop(reservation);

    let new_port = manager.reconfigure(requested_port, false).await.unwrap();

    assert_eq!(new_port, requested_port);
    assert_eq!(manager.state().await, (requested_port, false));
    assert_eq!(persisted_port(&state), format!("{requested_port}\n"));
    assert_eq!(persisted_public(&state), "0\n");
    assert!(accepts_connections(requested_port).await);
    assert!(wait_until_bindable(previous_port).await);
}

#[tokio::test]
async fn manager_reconfigure_bind_failure_keeps_the_previous_listener_and_config() {
    let dir = TestDir::new("web-proxy-manager-bind-failure");
    let state = dir.state(free_port());
    let manager = WebProxyManager::start(state.clone(), 0, false)
        .await
        .unwrap();
    let (previous_port, _) = manager.state().await;
    let previous_config = persisted_port(&state);
    let (_occupied, occupied_port) = occupied_listener();

    let result = manager.reconfigure(occupied_port, true).await;

    assert!(matches!(
        result,
        Err(ref error) if error.kind() == io::ErrorKind::AddrInUse
    ));
    assert_eq!(manager.state().await, (previous_port, false));
    assert_eq!(persisted_port(&state), previous_config);
    assert_eq!(persisted_public(&state), "0\n");
    assert!(accepts_connections(previous_port).await);
    assert!(!accepts_connections_at("127.0.0.2", previous_port).await);
}

#[tokio::test]
async fn manager_reconfigure_persist_failure_keeps_the_previous_listener_and_config() {
    let dir = TestDir::new("web-proxy-manager-persist-failure");
    let state = dir.state(free_port());
    let mut manager = WebProxyManager::start(state.clone(), 0, false)
        .await
        .unwrap();
    let (previous_port, _) = manager.state().await;
    let previous_config = persisted_port(&state);
    let blocked_parent = dir.0.join("not-a-directory");
    std::fs::write(&blocked_parent, b"block persistence").unwrap();
    manager.app.core.var_dir = blocked_parent.to_string_lossy().into_owned();

    assert!(manager.reconfigure(0, true).await.is_err());

    assert_eq!(manager.state().await, (previous_port, false));
    assert_eq!(persisted_port(&state), previous_config);
    assert_eq!(persisted_public(&state), "0\n");
    assert!(accepts_connections(previous_port).await);
    assert!(!accepts_connections_at("127.0.0.2", previous_port).await);
}

#[tokio::test]
async fn manager_reconfigure_public_persist_failure_restores_the_previous_port() {
    let dir = TestDir::new("web-proxy-manager-public-persist-failure");
    let state = dir.state(free_port());
    let manager = WebProxyManager::start(state.clone(), 0, false)
        .await
        .unwrap();
    let (previous_port, _) = manager.state().await;
    let blocker = std::path::Path::new(&state.core.var_dir)
        .join(format!("web_proxy_public.tmp.{}", std::process::id()));
    std::fs::write(blocker, b"block public persistence").unwrap();

    assert!(manager.reconfigure(0, true).await.is_err());

    assert_eq!(manager.state().await, (previous_port, false));
    assert_eq!(persisted_port(&state), format!("{previous_port}\n"));
    assert_eq!(persisted_public(&state), "0\n");
    assert!(accepts_connections(previous_port).await);
    assert!(!accepts_connections_at("127.0.0.2", previous_port).await);
}

fn api_request(method: Method, token: Option<&str>, body: Option<Value>) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri("/web-proxy/port");
    if let Some(token) = token {
        builder = builder.header("X-Supervisor-Token", token);
    }
    builder
        .body(body.map_or_else(Body::empty, |value| Body::from(value.to_string())))
        .unwrap()
}

async fn call_api(router: Router, request: Request<Body>) -> axum::response::Response {
    router.oneshot(request).await.unwrap()
}

async fn response_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap()
}

#[tokio::test]
async fn web_proxy_port_api_requires_token_and_reports_the_active_port() {
    let dir = TestDir::new("web-proxy-api-auth");
    let mut state = dir.state(free_port());
    let manager = WebProxyManager::start(state.clone(), 0, false)
        .await
        .unwrap();
    let (port, _) = manager.state().await;
    state.web_proxy = Some(Arc::new(manager));
    let router = api::build_router(state);

    for request in [
        api_request(Method::GET, None, None),
        api_request(Method::POST, None, Some(json!({"port": 0}))),
        api_request(Method::GET, Some("wrong-token"), None),
    ] {
        assert_eq!(
            call_api(router.clone(), request).await.status(),
            StatusCode::UNAUTHORIZED
        );
    }

    let response = call_api(router, api_request(Method::GET, Some("test-token"), None)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response_json(response).await,
        json!({"success": true, "port": port, "public": false})
    );
}

#[tokio::test]
async fn web_proxy_port_api_reports_disabled_when_no_manager_exists() {
    let dir = TestDir::new("web-proxy-api-disabled");
    let state = dir.state(free_port());
    let router = api::build_router(state.clone());

    for request in [
        api_request(Method::GET, Some("test-token"), None),
        api_request(Method::POST, Some("test-token"), Some(json!({"port": 0}))),
    ] {
        assert_eq!(
            call_api(router.clone(), request).await.status(),
            StatusCode::NOT_FOUND
        );
    }
    for name in ["web_proxy_port", "web_proxy_public"] {
        assert!(!std::path::Path::new(&state.core.var_dir)
            .join(name)
            .exists());
    }
}

#[tokio::test]
async fn web_proxy_port_api_rejects_invalid_and_conflicting_ports() {
    let dir = TestDir::new("web-proxy-api-rejections");
    let state = dir.state(free_port());
    let manager = WebProxyManager::start(state.clone(), 0, false)
        .await
        .unwrap();
    let (previous_port, _) = manager.state().await;
    let previous_config = persisted_port(&state);
    let mut router_state = state.clone();
    router_state.web_proxy = Some(Arc::new(manager.clone()));
    let router = api::build_router(router_state);

    for value in [
        json!({"port": 1}),
        json!({"port": 1023}),
        json!({"port": 65536}),
        json!({"port": -1}),
        json!({"port": "0"}),
        json!({"port": null}),
        json!({"port": 0, "public": "true"}),
    ] {
        let response = call_api(
            router.clone(),
            api_request(Method::POST, Some("test-token"), Some(value)),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    for reserved in [state.core.kernel.port(), state.core.supervisor_port] {
        let response = call_api(
            router.clone(),
            api_request(
                Method::POST,
                Some("test-token"),
                Some(json!({"port": reserved})),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    let (occupied, occupied_port) = loop {
        let (listener, port) = occupied_listener();
        if port != previous_port
            && port != state.core.kernel.port()
            && port != state.core.supervisor_port
        {
            break (listener, port);
        }
    };
    let response = call_api(
        router,
        api_request(
            Method::POST,
            Some("test-token"),
            Some(json!({"port": occupied_port, "public": true})),
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(manager.state().await, (previous_port, false));
    assert_eq!(persisted_port(&state), previous_config);
    assert_eq!(persisted_public(&state), "0\n");
    assert!(accepts_connections(previous_port).await);
    drop(occupied);
}

#[tokio::test]
async fn web_proxy_port_api_changes_access_and_preserves_it_when_omitted() {
    let dir = TestDir::new("web-proxy-api-switch");
    let mut state = dir.state(free_port());
    let manager = WebProxyManager::start(state.clone(), 0, false)
        .await
        .unwrap();
    let (mut previous_port, _) = manager.state().await;
    state.web_proxy = Some(Arc::new(manager.clone()));
    let router = api::build_router(state.clone());

    for (body, expected_public) in [
        (json!({"port": 0}), false),
        (json!({"port": 0, "public": true}), true),
        (json!({"port": 0}), true),
        (json!({"port": 0, "public": false}), false),
        (json!({"port": 0}), false),
    ] {
        let response = call_api(
            router.clone(),
            api_request(Method::POST, Some("test-token"), Some(body)),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let new_port = response_json(response).await["port"].as_u64().unwrap() as u16;
        assert_ne!(new_port, previous_port);
        assert_eq!(manager.state().await, (new_port, expected_public));
        assert_eq!(persisted_port(&state), format!("{new_port}\n"));
        assert_eq!(
            persisted_public(&state),
            if expected_public { "1\n" } else { "0\n" }
        );
        assert!(accepts_connections(new_port).await);
        assert_eq!(
            accepts_connections_at("127.0.0.2", new_port).await,
            expected_public
        );

        let response = call_api(
            router.clone(),
            api_request(Method::GET, Some("test-token"), None),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response_json(response).await,
            json!({"success": true, "port": new_port, "public": expected_public})
        );
        assert!(wait_until_bindable(previous_port).await);
        previous_port = new_port;
    }
}

use super::*;
use axum::{
    Json, Router,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use std::sync::{Arc, Mutex};

fn init_cli(config: PathBuf, token: PathBuf) -> Cli {
    Cli {
        json: true,
        config: Some(config),
        endpoint: Some("http://127.0.0.1:8741".into()),
        workspace: Some(Uuid::new_v4()),
        token_file: Some(token),
        command: Command::Init {
            local: true,
            force: false,
        },
    }
}

#[tokio::test]
async fn init_preserves_credentials_and_refuses_config_overwrite() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let token = directory.path().join("token");
    run(init_cli(config.clone(), token.clone())).await.unwrap();
    let original_config = fs::read(&config).unwrap();
    let original_token = fs::read(&token).unwrap();
    assert!(original_token.len() >= 32);
    let result = run(init_cli(config.clone(), token.clone()))
        .await
        .unwrap_err();
    assert_eq!(result.exit, 2);
    assert_eq!(fs::read(&config).unwrap(), original_config);
    assert_eq!(fs::read(&token).unwrap(), original_token);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&config).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&token).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::set_permissions(&token, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(secret_file(&token).unwrap_err().exit, 2);
    }
}

type Observed = Arc<Mutex<Vec<(HeaderMap, Value)>>>;
async fn mock_server(
    status: StatusCode,
    version: Option<&'static str>,
) -> (String, Observed, tokio::task::JoinHandle<()>) {
    let observed: Observed = Arc::default();
    let capture = observed.clone();
    let app = Router::new().route(
        "/v1/workspaces/{workspace}/mutations",
        post(move |headers: HeaderMap, Json(body): Json<Value>| {
            let capture = capture.clone();
            async move {
                capture.lock().unwrap().push((headers, body));
                let response = Json(if status.is_success() {
                    json!({"change_set_id":Uuid::new_v4(),"results":[]})
                } else {
                    json!({"error":{"code":"conflict","message":"stale version"}})
                });
                let mut headers = HeaderMap::new();
                if let Some(version) = version {
                    headers.insert("x-aq-graph-api-version", version.parse().unwrap());
                }
                (status, headers, response)
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (endpoint, observed, handle)
}

fn test_client(directory: &Path, endpoint: String) -> Client {
    let token = directory.join("token");
    private_write(&token, b"test-only-credential-0123456789abcdef", false).unwrap();
    Client::new(Config {
        endpoint,
        workspace: Uuid::new_v4(),
        token_file: Some(token),
    })
    .unwrap()
}

#[tokio::test]
async fn client_authenticates_negotiates_version_and_preserves_generated_retry_key() {
    let directory = tempfile::tempdir().unwrap();
    let (endpoint, observed, handle) = mock_server(StatusCode::CONFLICT, Some("1")).await;
    let client = test_client(directory.path(), endpoint);
    let error = client
        .mutation(vec![json!({"op":"example"})], None, None)
        .await
        .unwrap_err();
    assert_eq!(error.code, "conflict");
    assert_eq!(error.exit, 4);
    let captured = observed.lock().unwrap();
    assert_eq!(captured.len(), 1);
    assert_eq!(
        captured[0].0["authorization"],
        "Bearer test-only-credential-0123456789abcdef"
    );
    assert_eq!(captured[0].0["x-aq-graph-api-version"], "1");
    let key = captured[0].1["idempotency_key"].as_str().unwrap();
    assert_eq!(error.idempotency_key.as_deref(), Some(key));
    assert!(Uuid::parse_str(key).is_ok());
    handle.abort();
}

#[tokio::test]
async fn client_rejects_incompatible_success_and_keeps_explicit_retry_key() {
    let directory = tempfile::tempdir().unwrap();
    let (endpoint, _, handle) = mock_server(StatusCode::OK, Some("2")).await;
    let client = test_client(directory.path(), endpoint);
    let error = client
        .mutation(vec![], None, Some("known-retry-key".into()))
        .await
        .unwrap_err();
    assert_eq!(error.code, "protocol");
    assert_eq!(error.exit, 7);
    assert_eq!(error.idempotency_key.as_deref(), Some("known-retry-key"));
    handle.abort();
}

#[tokio::test]
async fn service_statuses_map_to_stable_exit_categories() {
    for (status, expected) in [
        (StatusCode::BAD_REQUEST, 2),
        (StatusCode::NOT_FOUND, 3),
        (StatusCode::CONFLICT, 4),
        (StatusCode::UNAUTHORIZED, 5),
        (StatusCode::FORBIDDEN, 6),
        (StatusCode::SERVICE_UNAVAILABLE, 7),
        (StatusCode::INTERNAL_SERVER_ERROR, 1),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let (endpoint, _, handle) = mock_server(status, Some("1")).await;
        let client = test_client(directory.path(), endpoint);
        let error = client
            .mutation(vec![], None, Some("test-retry-key".into()))
            .await
            .unwrap_err();
        assert_eq!(error.exit, expected, "HTTP {status}");
        assert_eq!(error.idempotency_key.as_deref(), Some("test-retry-key"));
        handle.abort();
    }
}

#[test]
fn known_migration_preflights_are_actionable_without_exposing_driver_details() {
    for (detail, expected) in [
        (
            "postgres://secret migration graph contract migration refused: multiple parents",
            "migration 0004",
        ),
        (
            "postgres://secret knowledge compatibility preflight failed: legacy verified",
            "migration 0005",
        ),
        (
            "postgres://secret unexpected driver detail",
            "schema/release compatibility",
        ),
    ] {
        let error = migration_failure(aq_graph::domain::Error::Internal(detail.into()));
        assert!(error.message.contains(expected));
        assert!(!error.message.contains("secret"));
        assert!(!error.message.contains("postgres://"));
    }
}

#[path = "wire_tests.rs"]
mod wire;

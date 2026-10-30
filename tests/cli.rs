//! Process-level JSON and exit-code contract. No database or network is required.
use std::process::Command;

#[test]
fn invalid_arguments_are_json_with_validation_exit() {
    let output = Command::new(env!("CARGO_BIN_EXE_aq-graph"))
        .args(["--json", "task", "show", "not-a-uuid"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(value["error"]["code"], "validation");
}

#[test]
fn unavailable_service_keeps_generated_mutation_key_in_json() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("client.json");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    std::fs::write(&config, serde_json::to_vec(&serde_json::json!({"endpoint":endpoint,"workspace":"00000000-0000-4000-8000-000000000001"})).unwrap()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_aq-graph"))
        .args([
            "--json",
            "--config",
            config.to_str().unwrap(),
            "task",
            "create",
            "--title",
            "Retry safe",
        ])
        .env("AQ_GRAPH_TOKEN", "test-only-process-credential-0123456789")
        .env_remove("AQ_GRAPH_TOKEN_FILE")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(7));
    assert!(output.stdout.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(value["error"]["code"], "unavailable");
    assert!(uuid::Uuid::parse_str(value["error"]["idempotency_key"].as_str().unwrap()).is_ok());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("test-only-process-credential"));
}

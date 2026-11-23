use super::*;

fn write_config(directory: &Path, endpoint: String, workspace: Uuid) -> PathBuf {
    let token = directory.join("token");
    private_write(&token, b"test-only-cli-credential-0123456789", false).unwrap();
    let config = directory.join("config.json");
    private_write(
        &config,
        &serde_json::to_vec(&Config {
            endpoint,
            workspace,
            token_file: Some(token),
        })
        .unwrap(),
        false,
    )
    .unwrap();
    config
}

fn parent_operations<'a>(record: &'a str, parent: &'a str) -> Vec<(Vec<&'a str>, Value)> {
    vec![
        (
            vec![
                "task",
                "set-parent",
                record,
                "--parent",
                parent,
                "--expected-version",
                "3",
            ],
            json!({"op":"set_parent","id":record,"expected_version":3,"parent_id":parent}),
        ),
        (
            vec![
                "task",
                "set-parent",
                record,
                "--root",
                "--expected-version",
                "3",
            ],
            json!({"op":"set_parent","id":record,"expected_version":3,"parent_id":null}),
        ),
    ]
}
fn knowledge_operations<'a>(record: &'a str, evidence: &'a Path) -> Vec<(Vec<&'a str>, Value)> {
    vec![
        (
            vec![
                "knowledge",
                "verify",
                record,
                "--expected-version",
                "3",
                "--evidence-file",
                evidence.to_str().unwrap(),
                "--reason",
                "Review evidence",
            ],
            json!({"op":"verify_node","id":record,"expected_version":3,"verification":"verified","evidence":read_json(evidence).unwrap()}),
        ),
        (
            vec![
                "knowledge",
                "retire",
                record,
                "--expected-version",
                "3",
                "--reason",
                "Replaced procedure",
            ],
            json!({"op":"update_node","id":record,"expected_version":3,"patch":{"retired":true,"retirement_reason":"Replaced procedure"}}),
        ),
    ]
}
fn link_operations<'a>(record: &'a str, parent: &'a str) -> Vec<(Vec<&'a str>, Value)> {
    vec![(
        vec![
            "link",
            "add",
            record,
            parent,
            "--type",
            "references",
            "--description",
            "Evidence connection",
        ],
        json!({"op":"add_edge","source":record,"target":parent,"edge_type":"references","source_revision":null,"target_revision":null,"description":"Evidence connection"}),
    )]
}
fn restore_operations(record: &str) -> Vec<(Vec<&str>, Value)> {
    let mut cases = Vec::new();
    for kind in ["knowledge", "task"] {
        for lifecycle in [false, true] {
            let mut args = vec![
                kind,
                "restore",
                record,
                "--expected-version",
                "3",
                "--revision",
                "1",
            ];
            if lifecycle {
                args.push("--include-lifecycle");
            }
            cases.push((args, json!({"op":"restore_node","id":record,"expected_version":3,"revision":1,"include_lifecycle":lifecycle})));
        }
    }
    cases
}
async fn assert_list_queries(config: &Path) {
    for kind in ["task", "knowledge"] {
        let result = run(Cli::try_parse_from([
            "aq-graph",
            "--config",
            config.to_str().unwrap(),
            kind,
            "list",
            "--valid-at",
            "2026-11-01T00:00:00Z",
            "--review-due-at",
            "2026-12-01T00:00:00Z",
            "--include-retired",
        ])
        .unwrap())
        .await
        .unwrap();
        let url = reqwest::Url::parse(&format!(
            "http://example.test/?{}",
            result["query"].as_str().unwrap()
        ))
        .unwrap();
        let query: std::collections::HashMap<_, _> = url.query_pairs().collect();
        assert_eq!(query["kind"], kind);
        assert_eq!(
            query["valid_at"],
            parse_timestamp("2026-11-01T00:00:00Z").unwrap()
        );
        assert_eq!(
            query["review_due_at"],
            parse_timestamp("2026-12-01T00:00:00Z").unwrap()
        );
        assert_eq!(query["include_retired"], "true");
    }
}
async fn assert_search_queries(config: &Path) {
    for kind in [None, Some("task"), Some("knowledge")] {
        let mut argv = vec![
            "aq-graph",
            "--config",
            config.to_str().unwrap(),
            "search",
            "authentication",
            "--valid-at",
            "2026-11-01T00:00:00Z",
        ];
        if let Some(kind) = kind {
            argv.extend(["--kind", kind]);
        }
        let result = run(Cli::try_parse_from(argv).unwrap()).await.unwrap();
        let url = reqwest::Url::parse(&format!(
            "http://example.test/?{}",
            result["query"].as_str().unwrap()
        ))
        .unwrap();
        let query: std::collections::HashMap<_, _> = url.query_pairs().collect();
        assert_eq!(
            query.get("kind").map(|v| v.as_ref()),
            kind,
            "default search leaves kind unfiltered"
        );
        assert_eq!(
            query["valid_at"],
            parse_timestamp("2026-11-01T00:00:00Z").unwrap()
        );
    }
}
async fn assert_revision_query(config: &Path, id: &str, workspace: Uuid) {
    let result = run(Cli::try_parse_from([
        "aq-graph",
        "--config",
        config.to_str().unwrap(),
        "revision",
        id,
        "--revision",
        "17",
    ])
    .unwrap())
    .await
    .unwrap();
    assert_eq!(
        result["path"],
        format!("/v1/workspaces/{}/nodes/{id}/revisions/17", workspace)
    );
    assert!(result["query"].is_null());
}
struct WireFixture {
    config: PathBuf,
    observed: Observed,
    record: String,
}
impl WireFixture {
    async fn assert_create(&self, kind: &str, evidence: &Path) {
        let mut argv = vec![
            "aq-graph",
            "--config",
            self.config.to_str().unwrap(),
            kind,
            "create",
            "--title",
            "Shared fields",
            "--type",
            "procedure",
            "--provenance-file",
            evidence.to_str().unwrap(),
            "--valid-from",
            "2026-01-01T00:00:00Z",
            "--valid-until",
            "2027-01-01T00:00:00Z",
            "--review-at",
            "2026-11-01T00:00:00Z",
        ];
        if kind == "task" {
            argv.extend(["--acceptance-criteria", "Task criterion"]);
        }
        run(Cli::try_parse_from(argv).unwrap()).await.unwrap();
        let operation = self.observed.lock().unwrap().last().unwrap().1["operations"][0].clone();
        assert_eq!(operation["kind"], kind);
        assert_eq!(operation["content_type"], "procedure");
        assert!(operation.get("knowledge_type").is_none());
        assert_eq!(operation["provenance"], read_json(evidence).unwrap());
        assert_eq!(
            operation["valid_from"],
            parse_timestamp("2026-01-01T00:00:00Z").unwrap()
        );
        assert_eq!(
            operation["valid_until"],
            parse_timestamp("2027-01-01T00:00:00Z").unwrap()
        );
        assert_eq!(
            operation["review_at"],
            parse_timestamp("2026-11-01T00:00:00Z").unwrap()
        );
        assert_eq!(
            operation.get("acceptance_criteria").is_some(),
            kind == "task"
        );
    }
    async fn assert_update(&self, kind: &str) {
        run(Cli::try_parse_from([
            "aq-graph",
            "--config",
            self.config.to_str().unwrap(),
            kind,
            "update",
            &self.record,
            "--expected-version",
            "5",
            "--type",
            "fact",
            "--clear-valid-from",
            "--valid-until",
            "2027-01-01T00:00:00Z",
            "--clear-review-at",
        ])
        .unwrap())
        .await
        .unwrap();
        let operation = self.observed.lock().unwrap().last().unwrap().1["operations"][0].clone();
        assert_eq!(
            operation,
            json!({"op":"update_node","id":self.record,"expected_version":5,"patch":{"content_type":"fact","valid_from":null,"valid_until":parse_timestamp("2027-01-01T00:00:00Z").unwrap(),"review_at":null}})
        );
    }
    async fn assert_verify(&self, kind: &str, evidence: &Path) {
        run(Cli::try_parse_from([
            "aq-graph",
            "--config",
            self.config.to_str().unwrap(),
            kind,
            "verify",
            &self.record,
            "--expected-version",
            "5",
            "--evidence-file",
            evidence.to_str().unwrap(),
            "--reason",
            "Reviewed evidence",
        ])
        .unwrap())
        .await
        .unwrap();
        assert_eq!(
            self.observed.lock().unwrap().last().unwrap().1["operations"][0],
            json!({"op":"verify_node","id":self.record,"expected_version":5,"verification":"verified","evidence":read_json(evidence).unwrap()})
        );
    }
    async fn assert_retire(&self, kind: &str, successor: &str) {
        for link in [false, true] {
            let mut argv = vec![
                "aq-graph",
                "--config",
                self.config.to_str().unwrap(),
                kind,
                "retire",
                &self.record,
                "--expected-version",
                "5",
                "--reason",
                "Replaced",
                "--successor",
                successor,
            ];
            if link {
                argv.push("--link-successor");
            }
            run(Cli::try_parse_from(argv).unwrap()).await.unwrap();
            let request = self.observed.lock().unwrap().last().unwrap().1.clone();
            let operations = request["operations"].as_array().unwrap();
            assert_eq!(operations.len(), if link { 2 } else { 1 });
            if link {
                assert_eq!(
                    operations[0],
                    json!({"op":"add_edge","source":successor,"target":self.record,"edge_type":"supersedes"})
                );
            }
            assert_eq!(
                operations.last().unwrap(),
                &json!({"op":"update_node","id":self.record,"expected_version":5,"patch":{"retired":true,"retirement_reason":"Replaced","successor_id":successor}})
            );
        }
    }
    async fn assert_summary(&self, summary: &Path) {
        for clear in [false, true] {
            let mut argv = vec![
                "aq-graph",
                "--config",
                self.config.to_str().unwrap(),
                "summary",
                &self.record,
                "--expected-version",
                "5",
                "--source-revision",
                "4",
            ];
            if clear {
                argv.push("--clear");
            } else {
                argv.extend(["--file", summary.to_str().unwrap()]);
            }
            run(Cli::try_parse_from(argv).unwrap()).await.unwrap();
            assert_eq!(
                self.observed.lock().unwrap().last().unwrap().1["operations"][0],
                json!({"op":"set_summary","id":self.record,"expected_version":5,"source_revision":4,"summary":if clear {Value::Null}else{json!("α\r\nexact summary")}})
            );
        }
    }
}
#[tokio::test]
async fn friendly_parent_verification_retirement_and_description_send_domain_operations() {
    let directory = tempfile::tempdir().unwrap();
    let (endpoint, observed, handle) = mock_server(StatusCode::OK, Some("1")).await;
    let config = write_config(directory.path(), endpoint, Uuid::new_v4());
    let evidence = directory.path().join("evidence.json");
    fs::write(&evidence, r#"[{"source_id":"review","kind":"url","url":"https://example.test/review","observed_at":"2026-10-30T00:00:00Z","retained":false}]"#).unwrap();
    let record = Uuid::new_v4().to_string();
    let parent = Uuid::new_v4().to_string();
    let mut cases = parent_operations(&record, &parent);
    cases.extend(knowledge_operations(&record, &evidence));
    cases.extend(restore_operations(&record));
    cases.extend(link_operations(&record, &parent));
    for (arguments, expected) in cases {
        let mut argv = vec!["aq-graph", "--config", config.to_str().unwrap()];
        argv.extend(arguments);
        run(Cli::try_parse_from(argv).unwrap()).await.unwrap();
        assert_eq!(
            observed.lock().unwrap().last().unwrap().1["operations"][0],
            expected
        );
    }
    handle.abort();
}

#[tokio::test]
async fn tree_and_search_flags_preserve_server_bounds_and_explicit_inclusion() {
    use axum::{extract::OriginalUri, routing::get};
    let app = Router::new().fallback(get(|OriginalUri(uri): OriginalUri| async move {
        (
            [("x-aq-graph-api-version", "1")],
            Json(json!({"path":uri.path(),"query":uri.query()})),
        )
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let directory = tempfile::tempdir().unwrap();
    let workspace = Uuid::new_v4();
    let config = write_config(directory.path(), endpoint, workspace);
    let id = Uuid::new_v4().to_string();
    for (arguments, expected) in [
        (
            vec![
                "task",
                "tree",
                &id,
                "--depth",
                "2",
                "--limit",
                "7",
                "--max-bytes",
                "8192",
            ],
            "depth=2&limit=7&max_bytes=8192",
        ),
        (
            vec![
                "search",
                "authentication",
                "--include-retired",
                "--include-disputed",
                "--limit",
                "7",
            ],
            "limit=7&q=authentication&include_retired=true&include_disputed=true",
        ),
    ] {
        let mut argv = vec!["aq-graph", "--config", config.to_str().unwrap()];
        argv.extend(arguments);
        let result = run(Cli::try_parse_from(argv).unwrap()).await.unwrap();
        assert_eq!(result["query"], expected);
    }
    assert_list_queries(&config).await;
    assert_search_queries(&config).await;
    assert_revision_query(&config, &id, workspace).await;
    handle.abort();
}

#[tokio::test]
async fn common_fields_verification_successors_and_summary_use_one_wire_contract() {
    let directory = tempfile::tempdir().unwrap();
    let (endpoint, observed, handle) = mock_server(StatusCode::OK, Some("1")).await;
    let config = write_config(directory.path(), endpoint, Uuid::new_v4());
    let evidence = directory.path().join("evidence.json");
    fs::write(&evidence,r#"[{"source_id":"evidence","kind":"url","url":"https://example.test/review","observed_at":"2026-10-30T00:00:00Z","retained":false}]"#).unwrap();
    let summary = directory.path().join("summary.txt");
    fs::write(&summary, "α\r\nexact summary").unwrap();
    let record = Uuid::new_v4().to_string();
    let successor = Uuid::new_v4().to_string();
    let fixture = WireFixture {
        config,
        observed,
        record,
    };
    for kind in ["task", "knowledge"] {
        fixture.assert_create(kind, &evidence).await;
        fixture.assert_update(kind).await;
        fixture.assert_verify(kind, &evidence).await;
        fixture.assert_retire(kind, &successor).await;
    }
    fixture.assert_summary(&summary).await;
    handle.abort();
}

#[test]
fn shared_field_args_reject_invalid_dates_conflicting_clears_and_task_only_knowledge_fields() {
    let id = Uuid::new_v4().to_string();
    for kind in ["task", "knowledge"] {
        assert!(
            Cli::try_parse_from([
                "aq-graph",
                kind,
                "create",
                "--title",
                "T",
                "--valid-from",
                "2026-01-01"
            ])
            .is_err()
        );
        for (value_flag, clear_flag) in [
            ("--valid-from", "--clear-valid-from"),
            ("--valid-until", "--clear-valid-until"),
            ("--review-at", "--clear-review-at"),
        ] {
            assert!(
                Cli::try_parse_from([
                    "aq-graph",
                    kind,
                    "update",
                    &id,
                    "--expected-version",
                    "1",
                    value_flag,
                    "2026-01-01T00:00:00Z",
                    clear_flag
                ])
                .is_err()
            );
        }
    }
    assert!(
        Cli::try_parse_from([
            "aq-graph",
            "knowledge",
            "create",
            "--title",
            "T",
            "--acceptance-criteria",
            "Task only"
        ])
        .is_err()
    );
    assert!(
        Cli::try_parse_from([
            "aq-graph",
            "summary",
            &id,
            "--expected-version",
            "1",
            "--source-revision",
            "1",
            "--clear",
            "--file",
            "summary.txt"
        ])
        .is_err()
    );
}

//! Differential coverage for bounded prefix and indexed fallback search branches.
use aq_graph::{domain::*, store::Store};
use serde_json::{Value, json};
use uuid::Uuid;

#[tokio::test]
async fn search_matches_full_reference_in_both_plan_modes() {
    let url = std::env::var("AQ_GRAPH_TEST_DATABASE_URL").expect("disposable PostgreSQL required");
    let store = Store::connect(&url, 1).await.unwrap();
    store.migrate().await.unwrap();
    let workspace = Uuid::new_v4();
    let token = Uuid::new_v4().to_string();
    store
        .bootstrap(workspace, "search-test", &token)
        .await
        .unwrap();
    let p = store.authenticate(&token, workspace).await.unwrap();
    // No revision rows are needed for this SQL fixture; remove nil on completion
    // so this database-required test can run repeatedly on the disposable DB.
    sqlx::query("INSERT INTO nodes(id,workspace_id,kind,version,content_revision,title,body,acceptance_criteria,task_state,content_type,verification,created_at,updated_at) VALUES ($1,$2,'task',1,1,'common nil','ordinary','','active','note','unverified',now(),now())")
        .bind(Uuid::nil()).bind(workspace).execute(&store.pool).await.unwrap();
    let base = Uuid::new_v4().as_u128() & !65535;
    for batch in 0..4 {
        let operations: Vec<Value> = (batch * 80..(batch + 1) * 80)
            .map(|i| {
                let mut op = json!({"op":"create_node","id":Uuid::from_u128(base+i),
                "kind":if i%2==0 {"task"} else {"knowledge"},
                "title":format!("common marker {i}"),
                "body":if i==300 {"rareword"} else if i%3==0 {"excluded"} else {"ordinary"}});
                if i % 2 == 0 {
                    op["task_state"] = json!("active");
                }
                op
            })
            .collect();
        store
            .mutate(
                &p,
                serde_json::from_value(
                    json!({"idempotency_key":Uuid::new_v4().to_string(),"operations":operations}),
                )
                .unwrap(),
            )
            .await
            .unwrap();
    }
    // Shared lifecycle/date fields exercise filters on both sides of the prefix boundary.
    sqlx::query("UPDATE nodes SET retired=(get_byte(uuid_send(id),15)%7=0), retirement_reason=CASE WHEN get_byte(uuid_send(id),15)%7=0 THEN 'archive' END, deleted=(get_byte(uuid_send(id),15)%31=0), verification=CASE WHEN get_byte(uuid_send(id),15)%11=0 THEN 'disputed' ELSE 'unverified' END, last_verified_by=CASE WHEN get_byte(uuid_send(id),15)%11=0 THEN 'search-test' END, last_verified_at=CASE WHEN get_byte(uuid_send(id),15)%11=0 THEN now() END, provenance=CASE WHEN get_byte(uuid_send(id),15)%11=0 THEN '[{\"source\":\"test\"}]'::jsonb ELSE '[]'::jsonb END, valid_from='2020-01-01', valid_until=CASE WHEN get_byte(uuid_send(id),15)%5=0 THEN '2021-01-01'::timestamptz END, review_at=CASE WHEN get_byte(uuid_send(id),15)%3=0 THEN '2022-01-01'::timestamptz END WHERE workspace_id=$1 AND id<>'00000000-0000-0000-0000-000000000000'::uuid")
        .bind(workspace).execute(&store.pool).await.unwrap();
    let live: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM nodes WHERE workspace_id=$1 AND NOT deleted ORDER BY id",
    )
    .bind(workspace)
    .fetch_all(&store.pool)
    .await
    .unwrap();
    let starts = [
        None,
        Some(Uuid::nil()),
        Some(Uuid::from_u128(base + 250)),
        Some(Uuid::from_u128(base + 255)),
        Some(Uuid::from_u128(base + 256)),
        Some(Uuid::from_u128(base + 319)),
        Some(Uuid::from_u128(base + 999)),
        Some(live[live.len() - 257]), // Exactly 256 remaining: exhausted prefix.
        Some(live[live.len() - 258]),
    ]; // Exactly 257: fallback may supply final row.
    let at = "2023-01-01T00:00:00Z".parse().unwrap();
    for mode in ["force_custom_plan", "force_generic_plan"] {
        sqlx::query(&format!("SET plan_cache_mode={mode}"))
            .execute(&store.pool)
            .await
            .unwrap();
        for filters in [
            NodeFilters::default(),
            NodeFilters {
                include_retired: true,
                include_disputed: true,
                ..Default::default()
            },
            NodeFilters {
                kind: Some(NodeKind::Task),
                valid_at: Some(at),
                ..Default::default()
            },
            NodeFilters {
                kind: Some(NodeKind::Knowledge),
                review_due_at: Some(at),
                ..Default::default()
            },
        ] {
            for term in [
                "common",
                "rareword",
                "missingword",
                "common -excluded",
                "-excluded",
                "the",
            ] {
                for limit in [1, 50, 200] {
                    for start in starts {
                        let expected:Vec<Uuid> = sqlx::query_scalar("SELECT id FROM nodes WHERE workspace_id=$1 AND NOT deleted AND ($2::text IS NULL OR kind=$2) AND ($3 OR NOT retired) AND ($4 OR verification IS DISTINCT FROM 'disputed') AND ($5::timestamptz IS NULL OR ((valid_from IS NULL OR valid_from <= $5) AND (valid_until IS NULL OR valid_until > $5))) AND ($6::timestamptz IS NULL OR (review_at IS NOT NULL AND review_at <= $6)) AND search_document @@ websearch_to_tsquery('english',$7) AND ($8::uuid IS NULL OR id>$8) ORDER BY id LIMIT $9")
                            .bind(workspace).bind(filters.kind.map(|k|k.as_str())).bind(filters.include_retired).bind(filters.include_disputed).bind(filters.valid_at).bind(filters.review_due_at).bind(term).bind(start).bind(i64::from(limit)+1).fetch_all(&store.pool).await.unwrap();
                        let page = store
                            .search_nodes(
                                &p,
                                term,
                                filters.clone(),
                                ListQuery {
                                    limit,
                                    cursor: start.map(|id| id.to_string()),
                                },
                            )
                            .await
                            .unwrap();
                        let expected_cursor = if expected.len() > limit as usize {
                            Some(expected[limit as usize - 1].to_string())
                        } else {
                            None
                        };
                        assert_eq!(
                            page.items.iter().map(|n| n.id).collect::<Vec<_>>(),
                            expected
                                .iter()
                                .take(limit as usize)
                                .copied()
                                .collect::<Vec<_>>(),
                            "{mode} {term} {filters:?} {start:?}"
                        );
                        assert_eq!(page.next_cursor, expected_cursor);
                    }
                }
            }
        }
    }
    sqlx::query("DELETE FROM nodes WHERE workspace_id=$1 AND id=$2")
        .bind(workspace)
        .bind(Uuid::nil())
        .execute(&store.pool)
        .await
        .unwrap();
}

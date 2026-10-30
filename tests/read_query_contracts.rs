//! Read pagination stays identical across custom/generic prepared plans and workspaces.
use aq_graph::{domain::*, store::Store};
use serde_json::{Value, json};
use uuid::Uuid;

struct Fixture {
    store: Store,
    p: Principal,
    ids: Vec<Uuid>,
}
impl Fixture {
    async fn new() -> Self {
        let url =
            std::env::var("AQ_GRAPH_TEST_DATABASE_URL").expect("disposable PostgreSQL required");
        let store = Store::connect(&url, 1).await.unwrap();
        store.migrate().await.unwrap();
        let workspace = Uuid::new_v4();
        let token = format!("read-contract-{}", Uuid::new_v4());
        store
            .bootstrap(workspace, "read-contract", &token)
            .await
            .unwrap();
        let p = store.authenticate(&token, workspace).await.unwrap();
        let f = Self {
            store,
            p,
            ids: (0..100).map(|_| Uuid::new_v4()).collect(),
        };
        let ops=f.ids.iter().enumerate().map(|(i,id)| {
            let state=if i==4 || i==16 {"draft"} else if i<20 {"active"} else if i<80 { ["active","draft","completed","canceled"][i%4] } else if i%2==0 {"completed"} else {"active"};
            json!({"op":"create_node","id":id,"kind":"task","title":format!("Node {i}"),"body":"payload ".repeat(256),"task_state":state})
        }).collect();
        f.apply(ops).await;
        let mut edges = Vec::new();
        for i in 10..80 {
            let parent = if i < 20 { i - 10 } else { 10 + (i - 20) % 10 };
            edges.push(json!({"op":"add_edge","source":f.ids[parent],"target":f.ids[i],"edge_type":"parent-child"}));
        }
        for i in 0..10 {
            edges.push(json!({"op":"add_edge","source":f.ids[80+i%2],"target":f.ids[20+i*4],"edge_type":"blocks"}));
        }
        f.apply(edges).await;
        for i in [0, 28] {
            let n = f.store.get_node(&f.p, f.ids[i]).await.unwrap();
            f.apply(vec![json!({"op":"update_node","id":n.id,"expected_version":n.version,"patch":{"retired":true,"retirement_reason":"Read contract archive"}})]).await;
        }
        f.apply(vec![
            json!({"op":"delete_node","id":f.ids[95],"expected_version":1}),
            json!({"op":"create_node","kind":"knowledge","title":"Never a ready task"}),
        ])
        .await;
        f
    }
    async fn apply(&self, ops: Vec<Value>) {
        self.store
            .mutate(
                &self.p,
                serde_json::from_value(
                    json!({"idempotency_key":Uuid::new_v4().to_string(),"operations":ops}),
                )
                .unwrap(),
            )
            .await
            .unwrap();
    }
    async fn mode(&self, mode: &str) {
        sqlx::query(&format!("SET plan_cache_mode={mode}"))
            .execute(&self.store.pool)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn ready_pages_preserve_leaf_lifecycle_prerequisite_and_scope_rules_for_both_plan_modes() {
    let f = Fixture::new().await;
    let foreign = Fixture::new().await;
    let mut expected: Vec<_> = [52, 68, 72]
        .into_iter()
        .chain((81..100).step_by(2).filter(|i| *i != 95))
        .map(|i| f.ids[i])
        .collect();
    expected.sort_unstable();
    for mode in ["force_custom_plan", "force_generic_plan"] {
        f.mode(mode).await;
        for limit in [1, 3, 200] {
            let mut cursor = None;
            let mut found = Vec::new();
            loop {
                let page = f
                    .store
                    .ready(&f.p, ListQuery { limit, cursor })
                    .await
                    .unwrap();
                assert!(page.items.iter().all(|n|n.workspace_id==f.p.workspace_id && !foreign.ids.contains(&n.id)));
                found.extend(page.items.into_iter().map(|n| n.id));
                cursor = page.next_cursor;
                if cursor.is_none() {
                    break;
                }
                assert!(found.len() <= expected.len(), "cursor must advance");
            }
            assert_eq!(found, expected, "{mode} limit {limit}");
        }
    }
}

#[tokio::test]
async fn history_cursor_pages_preserve_all_incident_events_without_cross_workspace_entries() {
    let f = Fixture::new().await;
    let foreign = Fixture::new().await;
    let id = f.ids[20];
    for i in 0..8 {
        let node = f.store.get_node(&f.p, id).await.unwrap();
        f.apply(vec![json!({"op":"update_node","id":id,"expected_version":node.version,"patch":{"body":format!("Edit {i}")}})]).await;
        foreign.apply(vec![json!({"op":"create_node","kind":"knowledge","title":"Interleaved other workspace"})]).await;
    }
    let expected:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(h)-'workspace_id'-'node_ids' FROM history_entries h WHERE workspace_id=$1 AND node_ids @> ARRAY[$2]::uuid[] ORDER BY sequence").bind(f.p.workspace_id).bind(id).fetch_all(&f.store.pool).await.unwrap();
    assert!(expected.len() > 10);
    for mode in ["force_custom_plan", "force_generic_plan"] {
        f.mode(mode).await;
        let mut cursor = None;
        let mut found = Vec::new();
        loop {
            let page = f
                .store
                .history(&f.p, id, ListQuery { limit: 3, cursor })
                .await
                .unwrap();
            found.extend(page["items"].as_array().unwrap().iter().cloned());
            cursor = page["next_cursor"].as_str().map(str::to_owned);
            if cursor.is_none() {
                break;
            }
            assert!(found.len() <= expected.len(), "history cursor must advance");
        }
        assert_eq!(found, expected, "{mode}");
        assert!(matches!(
            f.store.history(&foreign.p, id, ListQuery::default()).await,
            Err(Error::NotFound)
        ));
    }
}

//! Run an already-compiled child inside one actual UUID-owned disposable Graph DB.
#[path = "support/observation_test_db.rs"]
mod test_db;
use sqlx::{ConnectOptions, Connection};
#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let child = args.next().expect("compiled test executable required");
    // This wrapper is for the existing test cluster, never a canonical owner DB.
    let admin = std::env::var("DATABASE_URL").expect("test admin database required");
    let opts = admin
        .parse::<sqlx::postgres::PgConnectOptions>()
        .expect("test admin options");
    assert_eq!(opts.get_host(), "127.0.0.1");
    assert_eq!(opts.get_port(), 55432);
    assert_eq!(opts.get_database(), Some("postgres"));
    assert_eq!(opts.get_username(), "postgres");
    let db = test_db::TestDb::create().await;
    let options = db.options();
    let database = options.get_database().expect("owned database");
    let suffix = database
        .strip_prefix("aq_test_")
        .expect("owned UUID database prefix");
    assert_eq!(suffix.len(), 32);
    assert!(suffix.bytes().all(|n| n.is_ascii_hexdigit()));
    eprintln!(
        "Graph qualification owned_database={database} cluster=existing-test-55432 (no canonical database)"
    );
    let status = std::process::Command::new(child)
        .args(args)
        .env(
            "AQ_GRAPH_TEST_DATABASE_URL",
            options.to_url_lossy().as_str(),
        )
        .status()
        .expect("run actual compiled tests");
    let owned_name = database.to_owned();
    drop(db);
    let mut admin_conn = sqlx::PgConnection::connect(&admin)
        .await
        .expect("verify owned database cleanup");
    let remains: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_database WHERE datname=$1)")
            .bind(&owned_name)
            .fetch_one(&mut admin_conn)
            .await
            .expect("check exact owned database only");
    assert!(!remains, "owned qualification database was not removed");
    admin_conn.close().await.expect("close cleanup verifier");
    eprintln!("Graph qualification exact_owned_database_removed=true");
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
}

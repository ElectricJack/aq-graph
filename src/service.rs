//! Versioned HTTP transport. Authority is always obtained from the credential store.
use crate::{domain::*, store::Store};
use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{
        DefaultBodyLimit, Extension, Path, Query, State,
        rejection::{JsonRejection, QueryRejection},
    },
    http::{HeaderValue, Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;
use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;
use uuid::Uuid;

pub const MAX_REQUEST_BYTES: usize = 1_048_576;
pub const MAX_RESPONSE_BYTES: usize = 2_097_152;
pub const MAX_IN_FLIGHT: usize = 256;
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
struct HttpState {
    store: Store,
    admission: Arc<Semaphore>,
}

/// Construct the same application used by the server and integration clients.
pub fn router(store: Store) -> Router {
    let state = HttpState {
        store,
        admission: Arc::new(Semaphore::new(MAX_IN_FLIGHT)),
    };
    let authenticated = Router::new()
        .route("/nodes", get(list_nodes))
        .route("/nodes/{id}", get(get_node))
        .route("/nodes/{id}/context", get(context))
        .route("/nodes/{id}/tree", get(tree))
        .route("/nodes/{id}/history", get(history))
        .route("/nodes/{id}/revisions/{revision}", get(get_revision))
        .route("/nodes/{id}/diff", get(diff))
        .route("/ready", get(ready))
        .route("/search", get(search))
        .route("/mutations", post(mutate))
        .route("/changesets", post(create_draft))
        .route("/changesets/{id}", get(get_draft))
        .route("/changesets/{id}/validate", post(validate_draft))
        .route("/changesets/{id}/apply", post(apply_draft))
        .route("/credentials", post(grant))
        .route("/credentials/{actor}", delete(revoke))
        .route_layer(middleware::from_fn_with_state(state.clone(), authenticate));
    Router::new()
        .route(
            "/health/live",
            get(|| async { Json(json!({"status":"live", "api_version":"1"})) }),
        )
        .route("/health/ready", get(health_ready))
        .nest("/v1/workspaces/{workspace}", authenticated)
        .fallback(|| async { api_error(StatusCode::NOT_FOUND, "not_found", "Route not found") })
        .method_not_allowed_fallback(|| async {
            api_error(
                StatusCode::METHOD_NOT_ALLOWED,
                "method_not_allowed",
                "Method not allowed",
            )
        })
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BYTES))
        .layer(middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state)
}

fn api_error(status: StatusCode, code: &str, message: &str) -> Response {
    let mut response = (
        status,
        Json(json!({"error":{"code":code,"message":message}})),
    )
        .into_response();
    if status == StatusCode::UNAUTHORIZED {
        response.headers_mut().insert(
            "www-authenticate",
            HeaderValue::from_static("Bearer realm=\"aq-graph\""),
        );
    }
    if matches!(
        status,
        StatusCode::SERVICE_UNAVAILABLE | StatusCode::TOO_MANY_REQUESTS
    ) {
        response
            .headers_mut()
            .insert("retry-after", HeaderValue::from_static("1"));
    }
    response
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        match self {
            Error::Validation(message) => {
                api_error(StatusCode::BAD_REQUEST, "validation", &message)
            }
            Error::NotFound => api_error(StatusCode::NOT_FOUND, "not_found", "Record not found"),
            Error::Conflict(message) => api_error(StatusCode::CONFLICT, "conflict", &message),
            Error::Unauthorized => api_error(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "Valid workspace bearer credential required",
            ),
            Error::Forbidden => api_error(
                StatusCode::FORBIDDEN,
                "forbidden",
                "Insufficient capability",
            ),
            Error::Overloaded => api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "overloaded",
                "Service capacity exceeded; retry with backoff",
            ),
            Error::Database(sqlx::Error::PoolTimedOut | sqlx::Error::PoolClosed) => {
                Error::Overloaded.into_response()
            }
            Error::Database(sqlx::Error::Database(ref error))
                if matches!(
                    error.code().as_deref(),
                    Some("57014" | "55P03" | "40001" | "40P01")
                ) =>
            {
                Error::Overloaded.into_response()
            }
            Error::Database(_) | Error::Internal(_) => {
                // Driver messages may contain SQL, values, connection strings, or secrets.
                api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    "Internal service error",
                )
            }
        }
    }
}

async fn guard(State(state): State<HttpState>, request: Request<Body>, next: Next) -> Response {
    let started = Instant::now();
    let method = request.method().clone();
    let route = request
        .extensions()
        .get::<axum::extract::MatchedPath>()
        .map(|p| p.as_str().to_owned())
        .unwrap_or_else(|| "<unmatched>".into());
    let mut response = match state.admission.clone().try_acquire_owned() {
        Err(_) => Error::Overloaded.into_response(),
        Ok(_permit) => {
            if request
                .headers()
                .get("x-aq-graph-api-version")
                .is_some_and(|v| v != "1")
            {
                api_error(
                    StatusCode::BAD_REQUEST,
                    "unsupported_api_version",
                    "Supported API version is 1",
                )
            } else {
                match tokio::time::timeout(REQUEST_TIMEOUT, async {
                    let (parts, body) = request.into_parts();
                    let bytes = match to_bytes(body, MAX_REQUEST_BYTES).await {
                        Ok(bytes) => bytes,
                        Err(_) => {
                            return api_error(
                                StatusCode::PAYLOAD_TOO_LARGE,
                                "body_too_large",
                                "Request exceeds 1048576 bytes",
                            );
                        }
                    };
                    let response = next
                        .run(Request::from_parts(parts, Body::from(bytes)))
                        .await;
                    // Framework extractors (for example invalid UTF-8 in a path)
                    // can reject before a handler. Keep those errors in the same
                    // public envelope and never expose framework diagnostics.
                    if (response.status().is_client_error() || response.status().is_server_error())
                        && !response
                            .headers()
                            .get("content-type")
                            .is_some_and(|value| value.as_bytes().starts_with(b"application/json"))
                    {
                        return match response.status() {
                            StatusCode::BAD_REQUEST
                            | StatusCode::UNPROCESSABLE_ENTITY
                            | StatusCode::UNSUPPORTED_MEDIA_TYPE => api_error(
                                StatusCode::BAD_REQUEST,
                                "validation",
                                "Invalid request path or fields",
                            ),
                            StatusCode::NOT_FOUND => Error::NotFound.into_response(),
                            StatusCode::METHOD_NOT_ALLOWED => api_error(
                                StatusCode::METHOD_NOT_ALLOWED,
                                "method_not_allowed",
                                "Method not allowed",
                            ),
                            StatusCode::PAYLOAD_TOO_LARGE => api_error(
                                StatusCode::PAYLOAD_TOO_LARGE,
                                "body_too_large",
                                "Request exceeds server limit",
                            ),
                            _ => api_error(
                                StatusCode::INTERNAL_SERVER_ERROR,
                                "internal",
                                "Internal service error",
                            ),
                        };
                    }
                    let (parts, body) = response.into_parts();
                    match to_bytes(body, MAX_RESPONSE_BYTES).await {
                        Ok(bytes) => Response::from_parts(parts, Body::from(bytes)),
                        Err(_) => api_error(
                            StatusCode::PAYLOAD_TOO_LARGE,
                            "response_too_large",
                            "Response exceeds server limit; request a smaller page",
                        ),
                    }
                })
                .await
                {
                    Ok(response) => response,
                    Err(_) => api_error(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "timeout",
                        "Request deadline exceeded; retry mutations with the same idempotency key",
                    ),
                }
            }
        }
    };
    response
        .headers_mut()
        .insert("x-aq-graph-api-version", HeaderValue::from_static("1"));
    tracing::info!(method=%method, route=%route, status=response.status().as_u16(), duration_ms=started.elapsed().as_secs_f64()*1000.0,"request completed");
    response
}

async fn authenticate(
    State(state): State<HttpState>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    // Nesting preserves the original URI in OriginalUri.
    let uri = request
        .extensions()
        .get::<axum::extract::OriginalUri>()
        .map(|u| &u.0)
        .unwrap_or(request.uri());
    let workspace = uri
        .path()
        .split('/')
        .nth(3)
        .and_then(|s| Uuid::parse_str(s).ok());
    let Some(workspace) = workspace else {
        return Error::Validation("Invalid workspace UUID".into()).into_response();
    };
    let token = request
        .headers()
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.split_once(' '))
        .and_then(|(scheme, token)| scheme.eq_ignore_ascii_case("Bearer").then_some(token))
        .filter(|s| !s.is_empty() && s.len() <= 4096);
    let Some(token) = token else {
        return Error::Unauthorized.into_response();
    };
    match state.store.authenticate(token, workspace).await {
        Ok(principal) => {
            request.extensions_mut().insert(principal);
            next.run(request).await
        }
        Err(error) => error.into_response(),
    }
}

fn parse_json<T>(input: std::result::Result<Json<T>, JsonRejection>) -> Result<T> {
    input.map(|Json(value)| value).map_err(|e| {
        Error::Validation(if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
            "Request body exceeds 1048576 bytes".into()
        } else {
            "Invalid JSON body or fields".into()
        })
    })
}
fn parse_query<T>(input: std::result::Result<Query<T>, QueryRejection>) -> Result<T> {
    input
        .map(|Query(value)| value)
        .map_err(|_| Error::Validation("Invalid query parameters".into()))
}
#[derive(Deserialize)]
struct NodePath {
    id: String,
}
fn record_id(path: NodePath) -> Result<Uuid> {
    Uuid::parse_str(&path.id).map_err(|_| Error::Validation("Invalid record UUID".into()))
}

async fn health_ready(State(state): State<HttpState>) -> Response {
    // A compatible initialized schema is required, not merely a reachable TCP port.
    let expected = sqlx::migrate!("./migrations");
    let compatible = match sqlx::query(
        "SELECT version, checksum, success FROM _sqlx_migrations ORDER BY version",
    )
    .fetch_all(&state.store.pool)
    .await
    {
        Ok(rows) => {
            rows.len() == expected.iter().count()
                && rows.iter().zip(expected.iter()).all(|(row, migration)| {
                    row.get::<i64, _>("version") == migration.version
                        && row.get::<bool, _>("success")
                        && row.get::<Vec<u8>, _>("checksum").as_slice()
                            == migration.checksum.as_ref()
                })
        }
        Err(_) => false,
    };
    if compatible {
        Json(json!({"status":"ready","api_version":"1"})).into_response()
    } else {
        api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "not_ready",
            "Database or schema is unavailable",
        )
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NodesQuery {
    kind: Option<NodeKind>,
    limit: Option<u32>,
    cursor: Option<String>,
    #[serde(default)]
    include_retired: bool,
    valid_at: Option<DateTime<Utc>>,
    review_due_at: Option<DateTime<Utc>>,
}
impl NodesQuery {
    fn page(&self) -> ListQuery {
        ListQuery {
            limit: self.limit.unwrap_or(50),
            cursor: self.cursor.clone(),
        }
    }
    fn filters(self) -> NodeFilters {
        NodeFilters {
            kind: self.kind,
            include_retired: self.include_retired,
            include_disputed: false,
            valid_at: self.valid_at,
            review_due_at: self.review_due_at,
        }
    }
}
async fn list_nodes(
    State(s): State<HttpState>,
    Extension(p): Extension<Principal>,
    q: std::result::Result<Query<NodesQuery>, QueryRejection>,
) -> Result<Json<Page<Node>>> {
    let q = parse_query(q)?;
    let page = q.page();
    Ok(Json(s.store.list_filtered(&p, q.filters(), page).await?))
}
async fn get_node(
    State(s): State<HttpState>,
    Extension(p): Extension<Principal>,
    Path(path): Path<NodePath>,
) -> Result<Json<Node>> {
    Ok(Json(s.store.get_node(&p, record_id(path)?).await?))
}
async fn context(
    State(s): State<HttpState>,
    Extension(p): Extension<Principal>,
    Path(path): Path<NodePath>,
    q: std::result::Result<Query<ContextQuery>, QueryRejection>,
) -> Result<Json<Context>> {
    Ok(Json(
        s.store
            .context(&p, record_id(path)?, parse_query(q)?)
            .await?,
    ))
}
async fn tree(
    State(s): State<HttpState>,
    Extension(p): Extension<Principal>,
    Path(path): Path<NodePath>,
    q: std::result::Result<Query<ContextQuery>, QueryRejection>,
) -> Result<Json<Value>> {
    Ok(Json(
        s.store.tree(&p, record_id(path)?, parse_query(q)?).await?,
    ))
}
async fn ready(
    State(s): State<HttpState>,
    Extension(p): Extension<Principal>,
    q: std::result::Result<Query<ListQuery>, QueryRejection>,
) -> Result<Json<Page<Node>>> {
    Ok(Json(s.store.ready(&p, parse_query(q)?).await?))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchQuery {
    q: String,
    kind: Option<NodeKind>,
    limit: Option<u32>,
    cursor: Option<String>,
    #[serde(default)]
    include_retired: bool,
    #[serde(default)]
    include_disputed: bool,
    valid_at: Option<DateTime<Utc>>,
    review_due_at: Option<DateTime<Utc>>,
}
async fn search(
    State(s): State<HttpState>,
    Extension(p): Extension<Principal>,
    q: std::result::Result<Query<SearchQuery>, QueryRejection>,
) -> Result<Json<Page<Node>>> {
    let q = parse_query(q)?;
    let filters = NodeFilters {
        kind: q.kind,
        include_retired: q.include_retired,
        include_disputed: q.include_disputed,
        valid_at: q.valid_at,
        review_due_at: q.review_due_at,
    };
    Ok(Json(
        s.store
            .search_nodes(
                &p,
                &q.q,
                filters,
                ListQuery {
                    limit: q.limit.unwrap_or(50),
                    cursor: q.cursor,
                },
            )
            .await?,
    ))
}
async fn history(
    State(s): State<HttpState>,
    Extension(p): Extension<Principal>,
    Path(path): Path<NodePath>,
    q: std::result::Result<Query<ListQuery>, QueryRejection>,
) -> Result<Json<Value>> {
    Ok(Json(
        s.store
            .history(&p, record_id(path)?, parse_query(q)?)
            .await?,
    ))
}
#[derive(Deserialize)]
struct RevisionPath {
    id: String,
    revision: String,
}
async fn get_revision(
    State(s): State<HttpState>,
    Extension(p): Extension<Principal>,
    Path(path): Path<RevisionPath>,
) -> Result<Json<Value>> {
    let id =
        Uuid::parse_str(&path.id).map_err(|_| Error::Validation("Invalid record UUID".into()))?;
    let revision = path
        .revision
        .parse::<i64>()
        .ok()
        .filter(|revision| *revision > 0)
        .ok_or_else(|| Error::Validation("Revision must be a positive 64-bit integer".into()))?;
    Ok(Json(s.store.get_revision(&p, id, revision).await?))
}
#[derive(Deserialize)]
struct DiffQuery {
    from: i64,
    to: i64,
}
async fn diff(
    State(s): State<HttpState>,
    Extension(p): Extension<Principal>,
    Path(path): Path<NodePath>,
    q: std::result::Result<Query<DiffQuery>, QueryRejection>,
) -> Result<Json<Value>> {
    let q = parse_query(q)?;
    Ok(Json(
        s.store.diff(&p, record_id(path)?, q.from, q.to).await?,
    ))
}
async fn mutate(
    State(s): State<HttpState>,
    Extension(p): Extension<Principal>,
    body: std::result::Result<Json<MutationRequest>, JsonRejection>,
) -> Result<Json<MutationResult>> {
    Ok(Json(s.store.mutate(&p, parse_json(body)?).await?))
}
async fn create_draft(
    State(s): State<HttpState>,
    Extension(p): Extension<Principal>,
    body: std::result::Result<Json<DraftRequest>, JsonRejection>,
) -> Result<Json<Value>> {
    Ok(Json(s.store.create_draft(&p, parse_json(body)?).await?))
}
async fn get_draft(
    State(s): State<HttpState>,
    Extension(p): Extension<Principal>,
    Path(path): Path<NodePath>,
) -> Result<Json<Value>> {
    Ok(Json(s.store.get_draft(&p, record_id(path)?).await?))
}
async fn validate_draft(
    State(s): State<HttpState>,
    Extension(p): Extension<Principal>,
    Path(path): Path<NodePath>,
) -> Result<Json<Value>> {
    Ok(Json(s.store.validate_draft(&p, record_id(path)?).await?))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplyRequest {
    idempotency_key: String,
}
async fn apply_draft(
    State(s): State<HttpState>,
    Extension(p): Extension<Principal>,
    Path(path): Path<NodePath>,
    body: std::result::Result<Json<ApplyRequest>, JsonRejection>,
) -> Result<Json<MutationResult>> {
    let req = parse_json(body)?;
    Ok(Json(
        s.store
            .apply_draft(&p, record_id(path)?, &req.idempotency_key)
            .await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GrantRequest {
    actor_id: String,
    token: String,
    capability: Capability,
}
async fn grant(
    State(s): State<HttpState>,
    Extension(p): Extension<Principal>,
    body: std::result::Result<Json<GrantRequest>, JsonRejection>,
) -> Result<Json<Value>> {
    let req = parse_json(body)?;
    s.store
        .grant(&p, &req.actor_id, &req.token, req.capability)
        .await?;
    Ok(Json(json!({"ok":true})))
}
#[derive(Deserialize)]
struct ActorPath {
    actor: String,
}
async fn revoke(
    State(s): State<HttpState>,
    Extension(p): Extension<Principal>,
    Path(path): Path<ActorPath>,
) -> Result<Json<Value>> {
    s.store.revoke(&p, &path.actor).await?;
    Ok(Json(json!({"ok":true})))
}

/// Bind once, stop accepting on SIGINT/SIGTERM, and drain active requests.
pub async fn serve(store: Store, bind: SocketAddr) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(address=%listener.local_addr()?,"aq-graph listening");
    axum::serve(listener, router(store))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt;

    fn test_state(capacity: usize) -> HttpState {
        HttpState {
            store: Store {
                pool: sqlx::postgres::PgPoolOptions::new()
                    .connect_lazy("postgres://unused@127.0.0.1/unused")
                    .unwrap(),
            },
            admission: Arc::new(Semaphore::new(capacity)),
        }
    }
    #[tokio::test]
    async fn overload_is_immediate_structured_and_retriable() {
        let app = Router::new()
            .route("/", get(|| async { "unused" }))
            .layer(middleware::from_fn_with_state(test_state(0), guard));
        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers()["retry-after"], "1");
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap()["error"]["code"],
            "overloaded"
        );
    }
    #[tokio::test]
    async fn deadline_cancels_slow_handler_and_releases_admission() {
        let state = test_state(1);
        let app = Router::new()
            .route(
                "/",
                get(|| async {
                    tokio::time::sleep(REQUEST_TIMEOUT + Duration::from_secs(1)).await;
                    "late"
                }),
            )
            .layer(middleware::from_fn_with_state(state.clone(), guard));
        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers()["retry-after"], "1");
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap()["error"]["code"],
            "timeout"
        );
        assert_eq!(state.admission.available_permits(), 1);
    }
    #[tokio::test]
    async fn driver_details_are_hidden_and_capacity_errors_are_retriable() {
        let response = Error::Database(sqlx::Error::Protocol(
            "SELECT secret FROM credentials; password=private".into(),
        ))
        .into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            !text.contains("SELECT") && !text.contains("password") && !text.contains("credentials")
        );
        let response = Error::Database(sqlx::Error::PoolTimedOut).into_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers()["retry-after"], "1");
    }
    #[tokio::test]
    async fn unsupported_api_version_is_structured() {
        let app = Router::new()
            .route("/", get(|| async { "unused" }))
            .layer(middleware::from_fn_with_state(test_state(1), guard));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header("x-aq-graph-api-version", "2")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(response.headers()["x-aq-graph-api-version"], "1");
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap()["error"]["code"],
            "unsupported_api_version"
        );
    }
    #[tokio::test]
    async fn response_cap_returns_json_error() {
        let app = Router::new()
            .route("/", get(|| async { "x".repeat(MAX_RESPONSE_BYTES + 1) }))
            .layer(middleware::from_fn_with_state(test_state(1), guard));
        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap()["error"]["code"],
            "response_too_large"
        );
    }
}

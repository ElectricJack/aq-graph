use super::*;
use crate::observation::{CredentialIdentity, PullQuery, RegisterRequest, SnapshotRequest};
use axum::body::Bytes;
fn failure(error: Error) -> Response {
    match error {
        Error::Database(sqlx::Error::Database(ref d))
            if matches!(
                d.code().as_deref(),
                Some("40001" | "40P01" | "55P03" | "57014")
            ) =>
        {
            Error::Overloaded.into_response()
        }
        other => other.into_response(),
    }
}
fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}
pub(super) async fn capabilities(
    State(s): State<HttpState>,
    Extension(p): Extension<Principal>,
    Extension(c): Extension<CredentialIdentity>,
) -> Response {
    match s.store.observation_capabilities(&p, &c).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => failure(e),
    }
}
pub(super) async fn snapshot(
    State(s): State<HttpState>,
    Extension(p): Extension<Principal>,
    Extension(c): Extension<CredentialIdentity>,
    body: Bytes,
) -> Response {
    let req = match SnapshotRequest::decode(&body) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    match s.store.observation_snapshot(&p, &c, &req).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => failure(e),
    }
}
pub(super) async fn register(
    State(s): State<HttpState>,
    Extension(p): Extension<Principal>,
    Extension(c): Extension<CredentialIdentity>,
    body: Bytes,
) -> Response {
    let request = match RegisterRequest::decode(&body) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    match s
        .store
        .observation_register(&p, &c, &request, now_ms())
        .await
    {
        Ok(v) => Json(v).into_response(),
        Err(e) => failure(e),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SubscriptionPath {
    pub id: Uuid,
}
pub(super) async fn changes(
    State(s): State<HttpState>,
    Extension(p): Extension<Principal>,
    Extension(c): Extension<CredentialIdentity>,
    Path(path): Path<SubscriptionPath>,
    q: std::result::Result<Query<PullQuery>, QueryRejection>,
) -> Response {
    let q = match parse_query(q) {
        Ok(q) => q,
        Err(e) => return e.into_response(),
    };
    match s
        .store
        .observation_pull(&p, &c, path.id, &q, now_ms())
        .await
    {
        Ok(v) => Json(v).into_response(),
        Err(Error::NotFound) => api_error(
            StatusCode::GONE,
            "observation_receipt_not_retained",
            "The original observation receipt is unavailable; no replacement boundary was created",
        ),
        Err(e) => failure(e),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReleaseQuery {
    pub receipt_digest: String,
}
pub(super) async fn release(
    State(s): State<HttpState>,
    Extension(p): Extension<Principal>,
    Extension(c): Extension<CredentialIdentity>,
    Path(path): Path<SubscriptionPath>,
    q: std::result::Result<Query<ReleaseQuery>, QueryRejection>,
) -> Response {
    let q = match parse_query(q) {
        Ok(q) => q,
        Err(e) => return e.into_response(),
    };
    match s
        .store
        .observation_release(&p, &c, path.id, &q.receipt_digest)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => failure(e),
    }
}

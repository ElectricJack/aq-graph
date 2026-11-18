use super::*;
use crate::store::{TaskList, TaskListQuery};
use axum::http::HeaderMap;

pub(super) async fn read(
    State(state): State<HttpState>,
    headers: HeaderMap,
    input: std::result::Result<Json<TaskListQuery>, JsonRejection>,
) -> Result<Json<TaskList>> {
    let token = bearer(&headers).ok_or(Error::Unauthorized)?;
    Ok(Json(
        state.store.task_list(token, parse_json(input)?).await?,
    ))
}

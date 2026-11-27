use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, Request, State as ExtractState},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    routing::{get, post, put},
};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use uuid::Uuid;

use crate::{Result, capture::{Health, SensorState}, store::{Event, History, Store}};

#[derive(Clone)]
pub struct State {
    pub store: Arc<Store>,
    pub health: Arc<Health>,
    pub armed: Arc<watch::Sender<bool>>,
    pub push_configured: bool,
}

pub fn router(state: State) -> Router {
    Router::new()
        .route("/v1/pair", post(pair).delete(disconnect))
        .route("/v1/status", get(status))
        .route("/v1/armed", put(arm))
        .route("/v1/events", get(events))
        .route("/v1/events/{id}", get(event))
        .route("/v1/push-token", put(push_token))
        .layer(DefaultBodyLimit::max(4096))
        .layer(middleware::from_fn(|req: Request, next: Next| async move {
            let mut response = next.run(req).await;
            response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }))
        .with_state(state)
}

#[derive(Deserialize)]
struct Pair { code: String }

#[derive(Serialize)]
struct Credential { credential: String }

async fn pair(ExtractState(state): ExtractState<State>, Json(pair): Json<Pair>) -> ApiResult<Json<Credential>> {
    if pair.code.len() != 43 {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let credential = internal(state.store.pair(&pair.code))?.ok_or(StatusCode::UNAUTHORIZED)?;
    Ok(Json(Credential { credential }))
}

async fn disconnect(ExtractState(state): ExtractState<State>, headers: HeaderMap) -> ApiResult<StatusCode> {
    authorized_update(state.store.disconnect(bearer(&headers)?))
}

#[derive(Serialize)]
struct Status {
    armed: bool,
    camera: SensorState,
    microphone: SensorState,
    push_configured: bool,
    push_registered: bool,
}

async fn status(ExtractState(state): ExtractState<State>, headers: HeaderMap) -> ApiResult<Json<Status>> {
    authenticate(&state, &headers)?;
    let armed = internal(state.store.armed())?;
    let (camera, microphone) = state.health.states();
    Ok(Json(Status {
        armed, camera, microphone,
        push_configured: state.push_configured,
        push_registered: internal(state.store.has_push_token())?,
    }))
}

#[derive(Deserialize)]
struct Armed { armed: bool }

async fn arm(ExtractState(state): ExtractState<State>, headers: HeaderMap, Json(body): Json<Armed>) -> ApiResult<StatusCode> {
    let bearer = bearer(&headers)?;
    // Hold the watch write lock across the database update so concurrent requests
    // publish their capture state in database commit order.
    let mut outcome = Ok(false);
    state.armed.send_if_modified(|armed| {
        outcome = state.store.set_armed(bearer, body.armed);
        if matches!(outcome, Ok(true)) && *armed != body.armed {
            *armed = body.armed;
            return true;
        }
        false
    });
    authorized_update(outcome)
}

#[derive(Deserialize)]
struct Page { before: Option<i64>, limit: Option<usize> }

async fn events(ExtractState(state): ExtractState<State>, headers: HeaderMap, Query(page): Query<Page>) -> ApiResult<Json<History>> {
    authenticate(&state, &headers)?;
    Ok(Json(internal(state.store.history(page.before, page.limit.unwrap_or(50).clamp(1, 100)))?))
}

async fn event(ExtractState(state): ExtractState<State>, headers: HeaderMap, Path(id): Path<Uuid>) -> ApiResult<Json<Event>> {
    authenticate(&state, &headers)?;
    Ok(Json(internal(state.store.event(id))?.ok_or(StatusCode::NOT_FOUND)?))
}

#[derive(Deserialize)]
struct PushToken { token: String }

async fn push_token(ExtractState(state): ExtractState<State>, headers: HeaderMap, Json(body): Json<PushToken>) -> ApiResult<StatusCode> {
    if body.token.is_empty() || body.token.len() > 2048 || !body.token.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(StatusCode::BAD_REQUEST);
    }
    authorized_update(state.store.set_push_token(bearer(&headers)?, &body.token))
}

type ApiResult<T> = std::result::Result<T, StatusCode>;

fn bearer(headers: &HeaderMap) -> ApiResult<&str> {
    headers.get(header::AUTHORIZATION).and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer ")).filter(|value| value.len() == 43)
        .ok_or(StatusCode::UNAUTHORIZED)
}

fn authenticate(state: &State, headers: &HeaderMap) -> ApiResult<()> {
    if internal(state.store.authorized(bearer(headers)?))? { Ok(()) } else { Err(StatusCode::UNAUTHORIZED) }
}

fn authorized_update(result: Result<bool>) -> ApiResult<StatusCode> {
    if internal(result)? { Ok(StatusCode::NO_CONTENT) } else { Err(StatusCode::UNAUTHORIZED) }
}

fn internal<T>(result: Result<T>) -> ApiResult<T> {
    result.map_err(|error| {
        tracing::error!(%error, "Host request failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::{Body, to_bytes}, http::Method};
    use serde_json::{Value, json};
    use std::path::Path as FilePath;
    use tower::ServiceExt;
    use crate::store::Kind;

    #[tokio::test]
    async fn phone_lifecycle_and_event_history() {
        let store = Arc::new(Store::open(FilePath::new(":memory:")).unwrap());
        let (armed, watch) = watch::channel(false);
        let router = router(State {
            store: store.clone(),
            health: Arc::new(Health::default()),
            armed: Arc::new(armed),
            push_configured: true,
        });
        let code = store.issue_code().unwrap();
        let (status, _) = request(&router, Method::GET, "/v1/events", None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let path = "/v1/pair";
        let pair = json!({ "code": code });
        let (status, response) = request(&router, Method::POST, path, None, Some(&pair)).await;
        assert_eq!(status, StatusCode::OK);
        let credential = response["credential"].as_str();
        let (status, _) = request(&router, Method::POST, path, None, Some(&pair)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(store.issue_code().is_err());

        assert!(store.record(Kind::Motion, 0.2, true).unwrap().is_none());
        let (status, _) = request(&router, Method::PUT, "/v1/armed", credential, Some(&json!({"armed": true}))).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert!(*watch.borrow());

        let (status, _) = request(&router, Method::PUT, "/v1/push-token", credential, Some(&json!({"token": "test-token"}))).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let motion = store.record(Kind::Motion, 0.2, true).unwrap().unwrap();
        let sound = store.record(Kind::Sound, 18.0, true).unwrap().unwrap();
        assert!(store.pending().unwrap().is_some());

        let (status, newest) = request(&router, Method::GET, "/v1/events?limit=1", credential, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(newest["events"][0]["id"], sound.id);
        let cursor = newest["next_before"].as_i64().unwrap();
        let (_, older) = request(&router, Method::GET, &format!("/v1/events?limit=1&before={cursor}"), credential, None).await;
        assert_eq!(older["events"][0]["id"], motion.id);
        let (status, details) = request(&router, Method::GET, &format!("/v1/events/{}", motion.id), credential, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(details["kind"], "motion");

        let (status, _) = request(&router, Method::PUT, "/v1/armed", credential, Some(&json!({"armed": false}))).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert!(!*watch.borrow());
        assert!(store.record(Kind::Sound, 18.0, true).unwrap().is_none());

        let (status, _) = request(&router, Method::DELETE, "/v1/pair", credential, None).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert!(store.pending().unwrap().is_none());
        let (status, _) = request(&router, Method::GET, "/v1/status", credential, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(store.issue_code().is_ok());
    }

    async fn request(router: &Router, method: Method, uri: &str, credential: Option<&str>, body: Option<&Value>) -> (StatusCode, Value) {
        let mut request = Request::builder().method(method).uri(uri).header(header::CONTENT_TYPE, "application/json");
        if let Some(token) = credential {
            request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let body = body.map_or_else(Body::empty, |body| Body::from(body.to_string()));
        let response = router.clone().oneshot(request.body(body).unwrap()).await.unwrap();
        let status = response.status();
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }
}

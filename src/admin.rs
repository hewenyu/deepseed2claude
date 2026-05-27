use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use axum_extra::extract::CookieJar;
use axum_extra::extract::cookie::{Cookie, SameSite};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::store::{AdapterInput, KeyInput};
use crate::{AppState, Error};

const SESSION_COOKIE: &str = "deepseed_admin";

#[derive(Deserialize)]
pub struct LoginInput {
    username: String,
    password: String,
}

#[derive(Serialize)]
pub struct OkBody {
    ok: bool,
}

pub async fn login(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(input): Json<LoginInput>,
) -> Result<(CookieJar, Json<OkBody>), Error> {
    if input.username == state.config.admin_username && input.password == state.config.admin_password
    {
        let cookie = Cookie::build((SESSION_COOKIE, state.admin_session_token.to_string()))
            .path("/")
            .http_only(true)
            .same_site(SameSite::Lax)
            .build();
        Ok((jar.add(cookie), Json(OkBody { ok: true })))
    } else {
        Err(Error::Authentication(
            "invalid admin username or password".to_owned(),
        ))
    }
}

pub async fn logout(jar: CookieJar) -> (CookieJar, Json<OkBody>) {
    let cookie = Cookie::build((SESSION_COOKIE, ""))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .build();
    (jar.remove(cookie), Json(OkBody { ok: true }))
}

pub async fn session(State(state): State<AppState>, jar: CookieJar) -> Response {
    let authenticated = is_admin(&state, &jar);
    Json(json!({
        "authenticated": authenticated,
        "username": if authenticated { Some(state.config.admin_username.as_str()) } else { None }
    }))
    .into_response()
}

pub async fn state(State(state): State<AppState>, jar: CookieJar) -> Result<Response, Error> {
    require_admin(&state, &jar)?;
    Ok(Json(state.store.admin_state().await?).into_response())
}

pub async fn create_adapter(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(input): Json<AdapterInput>,
) -> Result<Response, Error> {
    require_admin(&state, &jar)?;
    Ok((StatusCode::CREATED, Json(state.store.create_adapter(input).await?)).into_response())
}

pub async fn update_adapter(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(id): Path<i64>,
    Json(input): Json<AdapterInput>,
) -> Result<Response, Error> {
    require_admin(&state, &jar)?;
    Ok(Json(state.store.update_adapter(id, input).await?).into_response())
}

pub async fn delete_adapter(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(id): Path<i64>,
) -> Result<Response, Error> {
    require_admin(&state, &jar)?;
    state.store.delete_adapter(id).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn create_client_key(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(input): Json<KeyInput>,
) -> Result<Response, Error> {
    require_admin(&state, &jar)?;
    Ok((
        StatusCode::CREATED,
        Json(state.store.create_client_key(input).await?),
    )
        .into_response())
}

pub async fn update_client_key(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(id): Path<i64>,
    Json(input): Json<KeyInput>,
) -> Result<Response, Error> {
    require_admin(&state, &jar)?;
    Ok(Json(state.store.update_client_key(id, input).await?).into_response())
}

pub async fn delete_client_key(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(id): Path<i64>,
) -> Result<Response, Error> {
    require_admin(&state, &jar)?;
    state.store.delete_client_key(id).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

fn require_admin(state: &AppState, jar: &CookieJar) -> Result<(), Error> {
    if is_admin(state, jar) {
        Ok(())
    } else {
        Err(Error::Authentication("admin login required".to_owned()))
    }
}

fn is_admin(state: &AppState, jar: &CookieJar) -> bool {
    jar.get(SESSION_COOKIE)
        .map(|cookie| cookie.value() == state.admin_session_token.as_ref())
        .unwrap_or(false)
}

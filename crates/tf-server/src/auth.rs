//! Server-issued opaque sessions. Only hashes are persisted; SSE uses the same HttpOnly cookie.
use crate::{api::AppState, jobs::now_millis};
use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const COOKIE: &str = "tf_session";

#[derive(Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    pub id: String,
    pub owner: String,
    pub last_seen: u64,
}

pub fn digest(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn credential(headers: &HeaderMap) -> Option<String> {
    if let Some(s) = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
    {
        return Some(s.to_string());
    }
    headers
        .get("cookie")
        .and_then(|v| v.to_str().ok())?
        .split(';')
        .find_map(|s| {
            let (key, value) = s.trim().split_once('=')?;
            (key == COOKIE).then(|| value.to_string())
        })
}

fn same_origin(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get("origin") else {
        return true;
    };
    let Some(host) = headers.get("host").and_then(|s| s.to_str().ok()) else {
        return false;
    };
    let Ok(origin) = origin.to_str() else {
        return false;
    };
    origin == format!("http://{host}") || origin == format!("https://{host}")
}

pub fn error(status: StatusCode, message: &str) -> Response {
    (status, Json(serde_json::json!({"error": message}))).into_response()
}

fn valid_host(st: &AppState, headers: &HeaderMap) -> bool {
    if !st.config.desktop {
        return true;
    }
    headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<axum::http::uri::Authority>().ok())
        .is_some_and(|authority| matches!(authority.host(), "127.0.0.1" | "localhost" | "[::1]"))
}

pub async fn open_session(State(st): State<AppState>, headers: HeaderMap) -> Response {
    if !valid_host(&st, &headers) {
        return error(StatusCode::FORBIDDEN, "桌面服务仅接受本机地址");
    }
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "不允许跨站创建会话");
    }
    let mut admission = st.admission.lock().await;
    if let Some(token) = credential(&headers) {
        if let Ok(Some(mut session)) = st.store.get_session(&digest(&token)) {
            if st.config.desktop
                || now_millis().saturating_sub(session.last_seen)
                    < st.config.session_ttl.as_millis() as u64
            {
                session.last_seen = now_millis();
                if !st
                    .store
                    .touch_session(&session.id, session.last_seen)
                    .unwrap_or(false)
                {
                    return error(StatusCode::INTERNAL_SERVER_ERROR, "会话保存失败");
                }
                return session_response(&st, &session, &token, &headers);
            }
        }
    }
    if !st.config.desktop {
        admission.retain(|&time| now_millis().saturating_sub(time) < 60_000);
        if admission.len() >= 32 {
            return error(StatusCode::TOO_MANY_REQUESTS, "会话创建频繁，请稍后重试");
        }
        match st.store.sessions() {
            Ok(s) if s.len() >= 128 => {
                return error(StatusCode::TOO_MANY_REQUESTS, "体验会话已满，请稍后再试")
            }
            Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "会话读取失败"),
            _ => {}
        }
        admission.push(now_millis());
    }
    let token = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let id = digest(&token);
    let session = SessionRecord {
        owner: if st.config.desktop {
            "local".into()
        } else {
            id.clone()
        },
        id,
        last_seen: now_millis(),
    };
    if st.store.put_session(&session).is_err() {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "会话保存失败");
    }
    session_response(&st, &session, &token, &headers)
}

fn session_response(
    st: &AppState,
    session: &SessionRecord,
    token: &str,
    headers: &HeaderMap,
) -> Response {
    let mut r = Json(serde_json::json!({"token": token, "mode": if st.config.desktop {"desktop"} else {"web"},
        "quality": st.config.quality, "max_patch_side": st.config.max_patch_side(),
        "expires_in_secs": if st.config.desktop {0} else {st.config.session_ttl.as_secs()}, "owner": session.owner})).into_response();
    let persistent = if st.config.desktop {
        "; Max-Age=2592000"
    } else {
        ""
    };
    // The reverse proxy terminates HTTPS. SameSite + origin validation protect mutations.
    let secure = if headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        == Some("https")
        || headers
            .get("origin")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("https://"))
    {
        "; Secure"
    } else {
        ""
    };
    let cookie = format!("{COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict{persistent}{secure}");
    r.headers_mut()
        .insert("set-cookie", HeaderValue::from_str(&cookie).unwrap());
    r.headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    r
}

pub async fn authorize(State(st): State<AppState>, mut req: Request<Body>, next: Next) -> Response {
    if !valid_host(&st, req.headers()) {
        return error(StatusCode::FORBIDDEN, "桌面服务仅接受本机地址");
    }
    if !same_origin(req.headers()) {
        return error(StatusCode::FORBIDDEN, "不允许跨站访问");
    }
    let path = req.uri().path().to_owned();
    if path.ends_with("/system/concurrency") {
        let valid = st
            .config
            .admin_token
            .as_ref()
            .zip(credential(req.headers()))
            .is_some_and(|(expected, got)| {
                let a = digest(expected);
                let b = digest(&got);
                a.bytes().zip(b.bytes()).fold(0u8, |v, (x, y)| v | (x ^ y)) == 0
            });
        return if valid {
            next.run(req).await
        } else {
            error(StatusCode::FORBIDDEN, "此操作需要管理员凭据")
        };
    }
    let lease = st.admission.lock().await;
    let Some(token) = credential(req.headers()) else {
        return error(StatusCode::UNAUTHORIZED, "请先建立会话");
    };
    let mut session = match st.store.get_session(&digest(&token)) {
        Ok(Some(s)) => s,
        _ => return error(StatusCode::UNAUTHORIZED, "会话无效或已结束"),
    };
    if !st.config.desktop
        && now_millis().saturating_sub(session.last_seen)
            >= st.config.session_ttl.as_millis() as u64
    {
        return error(StatusCode::UNAUTHORIZED, "会话已过期");
    }
    let parts: Vec<_> = path.split('/').filter(|s| !s.is_empty()).collect();
    // Router paths are relative to any deployment prefix.
    let api_index = parts.iter().position(|&s| s == "v1").unwrap_or(usize::MAX);
    let resource = parts.get(api_index.wrapping_add(1)).copied().unwrap_or("");
    let id = parts.get(api_index.wrapping_add(2)).copied();
    let world_id = match (resource, id) {
        ("worlds", Some(id)) => Some(id.to_string()),
        ("maps", Some(id)) => match st.store.get_map_node(id) {
            Ok(Some(m)) => Some(m.world_id),
            _ => return error(StatusCode::NOT_FOUND, "地图不存在"),
        },
        ("jobs", Some(id)) => match st.store.get_job(id) {
            Ok(Some(j)) => Some(j.world_id),
            _ => return error(StatusCode::NOT_FOUND, "作业不存在"),
        },
        _ => None,
    };
    if let Some(id) = world_id {
        match st.store.get_world(&id) {
            Ok(Some(w)) if w.owner_token == session.owner => {}
            _ => return error(StatusCode::NOT_FOUND, "资源不存在或不属于当前会话"),
        }
    }
    if now_millis().saturating_sub(session.last_seen) >= 30_000 {
        session.last_seen = now_millis();
        if !st
            .store
            .touch_session(&session.id, session.last_seen)
            .unwrap_or(false)
        {
            return error(StatusCode::UNAUTHORIZED, "会话已经结束");
        }
    }
    req.headers_mut().remove("x-tf-owner");
    req.headers_mut()
        .insert("x-tf-owner", HeaderValue::from_str(&session.owner).unwrap());
    req.extensions_mut().insert(session);
    drop(lease);
    let mut response = next.run(req).await;
    response
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    response
}

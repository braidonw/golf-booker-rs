//! HTTP-handler integration tests.
//!
//! These build the *real* router (via [`super::app::build_router`]) over an
//! in-memory database and drive it with `tower::ServiceExt::oneshot`, so the
//! login gate, the public/private split, redirects, and form validation are all
//! exercised end-to-end. Handlers that talk to a golf club (event browsing,
//! booking) are deliberately not covered here — they need a live club or an HTTP
//! mock; the club-free handlers are what this file pins down.

use super::app::{build_router, AppState};
use crate::config::Config;
use crate::email::Mailer;
use crate::scheduler::JobScheduler;
use crate::test_support::test_pool;
use axum::body::Body;
use axum::http::header::{CONTENT_TYPE, COOKIE, LOCATION, SET_COOKIE};
use axum::http::{Request, StatusCode};
use axum::response::Response;
use axum::Router;
use sqlx::SqlitePool;
use std::sync::Arc;
use tower::ServiceExt;

/// Build the app router over a given in-memory pool, with email disabled,
/// dry-run on, and insecure cookies (so the session cookie round-trips in the
/// test harness without TLS).
async fn test_router(db: &SqlitePool) -> Router {
    let config = Config {
        database_url: "sqlite::memory:".to_string(),
        port: 0,
        dry_run: true,
        cookie_secure: false,
        base_url: "http://localhost".to_string(),
        smtp: None,
    };
    let mailer = Mailer::from_config(None).unwrap();
    let scheduler = JobScheduler::new(db.clone(), true, mailer.clone(), config.base_url.clone());
    let state = Arc::new(AppState::for_test(db.clone(), config, scheduler, mailer));
    build_router(state).await.expect("build router")
}

fn get(uri: &str) -> Request<Body> {
    Request::builder().uri(uri).body(Body::empty()).unwrap()
}

fn get_authed(uri: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header(COOKIE, cookie)
        .body(Body::empty())
        .unwrap()
}

fn post_form(uri: &str, cookie: Option<&str>, body: &str) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(uri)
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded");
    if let Some(cookie) = cookie {
        builder = builder.header(COOKIE, cookie);
    }
    builder.body(Body::from(body.to_string())).unwrap()
}

async fn send(router: &Router, req: Request<Body>) -> Response {
    router.clone().oneshot(req).await.unwrap()
}

async fn body_text(resp: Response) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// The session cookie set by a successful login, as a `name=value` pair to send
/// back on subsequent requests.
fn session_cookie(resp: &Response) -> String {
    resp.headers()
        .get(SET_COOKIE)
        .expect("a successful login sets a session cookie")
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

/// Seed an account and log in, returning the session cookie.
async fn login(router: &Router, db: &SqlitePool, username: &str, password: &str) -> String {
    crate::users::create(
        db,
        username,
        Some(&format!("{username}@example.com")),
        password,
    )
    .await
    .unwrap();
    let resp = send(
        router,
        post_form(
            "/login",
            None,
            &format!("username={username}&password={password}"),
        ),
    )
    .await;
    assert!(
        resp.status().is_redirection(),
        "login should redirect on success, got {}",
        resp.status()
    );
    session_cookie(&resp)
}

#[tokio::test]
async fn health_is_public() {
    let db = test_pool().await;
    let router = test_router(&db).await;
    let resp = send(&router, get("/health")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(body_text(resp).await, "ok");
}

#[tokio::test]
async fn index_redirects_to_login_when_unauthenticated() {
    let db = test_pool().await;
    let router = test_router(&db).await;
    let resp = send(&router, get("/")).await;
    assert!(resp.status().is_redirection());
    let location = resp.headers().get(LOCATION).unwrap().to_str().unwrap();
    assert!(location.starts_with("/login"), "got: {location}");
}

#[tokio::test]
async fn login_page_is_public() {
    let db = test_pool().await;
    let router = test_router(&db).await;
    let resp = send(&router, get("/login")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(body_text(resp).await.to_lowercase().contains("password"));
}

#[tokio::test]
async fn bad_login_is_rejected_with_a_message() {
    let db = test_pool().await;
    let router = test_router(&db).await;
    crate::users::create(&db, "badlogin", None, "rightpassword")
        .await
        .unwrap();

    let resp = send(
        &router,
        post_form("/login", None, "username=badlogin&password=wrongpassword"),
    )
    .await;
    // Re-renders the login page (200) rather than redirecting.
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(body_text(resp)
        .await
        .contains("Invalid username or password"));
}

#[tokio::test]
async fn good_login_unlocks_the_home_page() {
    let db = test_pool().await;
    let router = test_router(&db).await;
    let cookie = login(&router, &db, "alice", "password1").await;

    let resp = send(&router, get_authed("/", &cookie)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(body_text(resp).await.contains("alice"));
}

#[tokio::test]
async fn protected_page_requires_auth() {
    let db = test_pool().await;
    let router = test_router(&db).await;
    // No cookie -> redirected to login.
    let resp = send(&router, get("/users")).await;
    assert!(resp.status().is_redirection());

    // With a session -> served.
    let cookie = login(&router, &db, "bob", "password1").await;
    let resp = send(&router, get_authed("/users", &cookie)).await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn creating_a_user_with_a_short_password_is_rejected() {
    let db = test_pool().await;
    let router = test_router(&db).await;
    let cookie = login(&router, &db, "carol", "password1").await;

    let resp = send(
        &router,
        post_form(
            "/users",
            Some(&cookie),
            "username=newbie&email=n@example.com&password=short",
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(body_text(resp).await.contains("at least 8 characters"));
}

#[tokio::test]
async fn cannot_delete_your_own_account() {
    let db = test_pool().await;
    let router = test_router(&db).await;
    let id = crate::users::create(&db, "dave", Some("d@example.com"), "password1")
        .await
        .unwrap();
    let resp = send(
        &router,
        post_form("/login", None, "username=dave&password=password1"),
    )
    .await;
    let cookie = session_cookie(&resp);

    let resp = send(
        &router,
        post_form(&format!("/users/{id}/delete"), Some(&cookie), ""),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    // The apostrophe in "can't" is HTML-escaped by Askama, so match the rest.
    assert!(body_text(resp).await.contains("delete your own account"));
    // The account still exists.
    assert_eq!(crate::users::count(&db).await.unwrap(), 1);
}

#[tokio::test]
async fn creating_a_club_with_a_bad_timezone_is_rejected() {
    let db = test_pool().await;
    let router = test_router(&db).await;
    let cookie = login(&router, &db, "erin", "password1").await;

    let resp = send(
        &router,
        post_form(
            "/clubs",
            Some(&cookie),
            "name=Test&base_url=https://x.test&username=u&password=p&member_id=m&timezone=Mars/Phobos",
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(body_text(resp).await.contains("Unknown timezone"));
    // Nothing was persisted.
    assert!(crate::clubs::list(&db).await.unwrap().is_empty());
}

#[tokio::test]
async fn scheduled_jobs_page_renders_for_authed_user() {
    let db = test_pool().await;
    let router = test_router(&db).await;
    let cookie = login(&router, &db, "frank", "password1").await;

    let resp = send(&router, get_authed("/scheduled-jobs", &cookie)).await;
    assert_eq!(resp.status(), StatusCode::OK);
}

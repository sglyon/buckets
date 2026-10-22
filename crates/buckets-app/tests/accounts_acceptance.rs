//! Account orchestration remains independent of native rendering and live auth.
#[path = "../src/account_state.rs"]
mod account_state;

use account_state::{AccountSessions, external_url};
use buckets_plugin::AccountState;
use std::sync::atomic::Ordering;

#[test]
fn a_replaced_sign_in_cannot_publish_or_open_its_browser() {
    let mut sessions = AccountSessions::default();
    let (first, cancelled) = sessions.begin("ai");
    let (second, _) = sessions.begin("ai");
    assert!(cancelled.load(Ordering::Acquire));
    assert!(!sessions.accepts("ai", first));
    assert!(sessions.accepts("ai", second));
    assert!(!sessions.complete("ai", first, Ok(AccountState::default())));
    assert!(sessions.busy("ai"));
    assert!(sessions.complete("ai", second, Ok(AccountState::default())));
    assert!(!sessions.busy("ai"));
}

#[test]
fn close_and_plugin_reload_cancel_in_flight_connections() {
    let mut sessions = AccountSessions::default();
    let (generation, cancelled) = sessions.begin("ai");
    sessions.cancel_all();
    assert!(cancelled.load(Ordering::Acquire));
    assert!(!sessions.accepts("ai", generation));
    assert!(!sessions.public["ai"].busy);
    let (generation, cancelled) = sessions.begin("ai");
    sessions.clear();
    assert!(cancelled.load(Ordering::Acquire));
    assert!(!sessions.accepts("ai", generation));
    assert!(sessions.public.is_empty());
}

#[test]
fn failed_refresh_keeps_last_public_account_and_surfaces_recovery() {
    let mut sessions = AccountSessions::default();
    let (generation, _) = sessions.begin("ai");
    let connected = AccountState {
        active_account_id: Some("account-a".into()),
        ..Default::default()
    };
    sessions.complete("ai", generation, Ok(connected));
    let (generation, _) = sessions.begin("ai");
    sessions.progress("ai", generation, "Refreshing models…".into());
    assert_eq!(sessions.public["ai"].message, "Refreshing models…");
    sessions.complete("ai", generation, Err("Reconnect to refresh models.".into()));
    let presentation = &sessions.public["ai"];
    assert!(presentation.error);
    assert_eq!(
        presentation
            .state
            .as_ref()
            .unwrap()
            .active_account_id
            .as_deref(),
        Some("account-a")
    );
    assert_eq!(presentation.message, "Reconnect to refresh models.");
}

#[test]
fn closing_the_owner_cancels_every_account_service() {
    let cancelled = {
        let mut sessions = AccountSessions::default();
        sessions.begin("ai").1
    };
    assert!(cancelled.load(Ordering::Acquire));
}

#[test]
fn external_account_actions_reject_credentials_and_unsafe_urls() {
    for unsafe_url in [
        "http://auth.openai.com/login",
        "javascript:alert(1)",
        "https://token@auth.openai.com/login",
        "https://auth.openai.com/#token",
        "https://auth.openai.com/login\nsecret",
        "https://auth.openai.com/a b",
    ] {
        assert!(external_url(unsafe_url).is_err(), "{unsafe_url}");
    }
    assert!(external_url("https://auth.openai.com/api/accounts/authorize?state=opaque").is_ok());
    assert!(external_url("https://chatgpt.com/settings/usage").is_ok());
}

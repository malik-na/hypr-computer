//! The user's desktop as the live views need it, when it is Hyprland: what
//! Hyprland is asked, checked against a fake Hyprland.
#![forbid(unsafe_code)]

mod support;

use std::ffi::OsStr;
use std::path::PathBuf;
use std::sync::Arc;

use ports::{Desktop, DesktopError};
use support::Env;
use support::hyprland::{Hyprland, PLACE_VIEWS, SHOW_VIEWS, SIGNATURE};

/// A fake Hyprland, and the desktop the daemon would make of a session
/// whose `$WAYLAND_DISPLAY` is next to it.
fn hyprland(env: &Env) -> (Hyprland, Arc<dyn Desktop>) {
    let dir = env.path("desktop");
    let hyprland = Hyprland::start(&dir);
    let desktop = adapters::desktop(Some(&dir.join("wayland-1")), Some(OsStr::new(SIGNATURE)));
    (hyprland, desktop)
}

#[tokio::test]
async fn placing_the_views_has_hyprland_keep_them_on_workspace_10_and_out_of_focus() {
    let env = Env::new();
    let (hyprland, desktop) = hyprland(&env);
    desktop.place_views().await.unwrap();
    desktop.place_views().await.unwrap();
    // The same rule each time: Hyprland updates it in place.
    assert_eq!(hyprland.requests(), [PLACE_VIEWS, PLACE_VIEWS]);
}

#[tokio::test]
async fn showing_the_views_takes_the_user_to_workspace_10_and_nothing_else() {
    let env = Env::new();
    let (hyprland, desktop) = hyprland(&env);
    assert_eq!(desktop.show_views().await.unwrap(), "workspace 10");
    assert_eq!(hyprland.requests(), [SHOW_VIEWS]);
}

#[tokio::test]
async fn a_refusal_from_hyprland_is_an_error_that_says_what_it_said() {
    let env = Env::new();
    let (hyprland, desktop) = hyprland(&env);
    hyprland.answer_with("error: no such dispatcher");
    let refused = desktop.show_views().await.unwrap_err();
    assert!(
        refused.0.ends_with(": error: no such dispatcher"),
        "{refused}"
    );
}

#[tokio::test]
async fn a_hyprland_that_is_gone_is_an_error_naming_where_it_was_looked_for() {
    let env = Env::new();
    let desktop = adapters::desktop(
        Some(&env.path("desktop").join("wayland-1")),
        Some(OsStr::new(SIGNATURE)),
    );
    let gone = desktop.place_views().await.unwrap_err();
    let socket: PathBuf = env.path("desktop/hypr/fake-hyprland/.socket.sock");
    assert!(gone.0.contains(&socket.display().to_string()), "{gone}");
}

#[tokio::test]
async fn views_are_refused_on_a_desktop_that_isnt_hyprland_saying_why() {
    let env = Env::new();
    let socket = env.path("desktop").join("wayland-1");
    let not_hyprland = adapters::desktop(Some(&socket), None);
    let no_desktop = adapters::desktop(None, Some(OsStr::new(SIGNATURE)));
    assert_eq!(
        not_hyprland.place_views().await,
        Err(DesktopError(
            "live views need Hyprland for now, and HYPRLAND_INSTANCE_SIGNATURE isn't set"
                .to_owned()
        ))
    );
    assert_eq!(
        no_desktop.show_views().await,
        Err(DesktopError(
            "there is no desktop to show it on: WAYLAND_DISPLAY isn't set".to_owned()
        ))
    );
}

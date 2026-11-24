//! #26, part 3: a page's popups (a `<select>` dropdown, the right-click
//! menu) appear above the page on the agent's screen and take clicks.
//! Written from the spec alone.
//!
//! The tests pass with a SKIPPED note where no Chromium-family browser is
//! installed (or `HC26_BROWSER=none`); every test first checks that the agent
//! has `open_url`.
#![forbid(unsafe_code)]

mod spec26;

use std::time::Duration;

use serde_json::json;
use spec26::web::Page;
use spec26::{Agent, BLUE, CYAN, GREEN, MAGENTA, ORANGE, centre, css};

/// The page's colour for each option of its `<select>`.
const CHOICES: [[u8; 3]; 4] = [MAGENTA, ORANGE, BLUE, CYAN];

fn select_page() -> Page {
    let colours: Vec<String> = CHOICES.iter().map(|c| format!("'{}'", css(*c))).collect();
    Page::html(format!(
        "<!doctype html><html><head><title>Choose</title></head>\
         <body style=\"margin:0;height:100vh;background:{bg}\">\
         <select id=\"s\" style=\"position:absolute;left:200px;top:120px;width:420px;\
           height:64px;font-size:32px;background:{green};color:black;border:0\">\
         <option>one</option><option>two</option><option>three</option><option>four</option>\
         </select>\
         <script>s.onchange = () => {{\
           document.body.style.background = [{colours}][s.selectedIndex];\
         }};</script></body></html>",
        bg = css(MAGENTA),
        green = css(GREEN),
        colours = colours.join(","),
    ))
}

#[test]
fn a_select_dropdown_opens_above_the_page_and_takes_a_click() {
    let mut a = Agent::start();
    if !a.has_browser() {
        return;
    }
    a.web.page("/choose", select_page());
    a.shim.open(&a.web.url("/choose"));
    let page = a.shim.screenshot();
    assert!(page.shows(MAGENTA), "the page isn't shown: {page:?}");
    let found = page
        .bbox(GREEN)
        .unwrap_or_else(|| panic!("the <select> isn't on the screen: {page:?}"));
    let (x, y) = centre(found);
    // The select itself, with room for a focus ring.
    let select = (found.0 - 8, found.1 - 8, found.2 + 8, found.3 + 8);

    a.shim.ok("click", json!({"x": x, "y": y}));
    let (opened, open) = a.shim.screenshot_until(Duration::from_secs(5), |shot| {
        shot.changed_box(&page, select).is_some_and(|(l, t, r, b)| {
            // A list of four options, below or above the select.
            r - l > 100 && b - t > 80
        })
    });
    assert!(
        opened,
        "clicking the <select> showed no dropdown above the page: {open:?}"
    );
    let (l, t, r, b) = open.changed_box(&page, select).unwrap();
    // The list's four options share its height; click the third.
    let (ox, oy) = ((l + r) / 2, t + (b - t) * 5 / 8);
    a.shim.ok("click", json!({"x": ox, "y": oy}));
    let (chose, shot) = a
        .shim
        .screenshot_until(Duration::from_secs(5), |shot| shot.shows(BLUE));
    assert!(
        chose,
        "clicking the third option in the dropdown (at {ox},{oy}, in the list at \
         {l},{t}-{r},{b}) didn't choose it: {shot:?}"
    );
    let (closed, shot) = a.shim.screenshot_until(Duration::from_secs(5), |shot| {
        shot.share_in((l, t, r, b), BLUE) > 0.9
    });
    assert!(
        closed,
        "the dropdown stayed open after the choice: {shot:?}"
    );
}

#[test]
fn a_right_click_menu_opens_above_the_page_and_a_click_outside_closes_it() {
    let mut a = Agent::start();
    if !a.has_browser() {
        return;
    }
    a.web.page("/page", Page::solid("Page", ORANGE, ""));
    a.shim.open(&a.web.url("/page"));
    let page = a.shim.screenshot();
    assert!(page.shows(ORANGE), "the page isn't shown: {page:?}");
    let (l, t, r, b) = page.bbox(ORANGE).unwrap();
    // Near the top left of the page, so the menu opens down and right.
    let (x, y) = (l + 100, t + 60);

    a.shim
        .ok("click", json!({"x": x, "y": y, "button": "right"}));
    let nowhere = (0, 0, 0, 0);
    let (opened, menu) = a.shim.screenshot_until(Duration::from_secs(5), |shot| {
        shot.changed_box(&page, nowhere)
            .is_some_and(|(ml, mt, mr, mb)| mr - ml > 100 && mb - mt > 100)
    });
    assert!(opened, "a right click on the page showed no menu: {menu:?}");
    let (ml, mt, mr, mb) = menu.changed_box(&page, nowhere).unwrap();
    assert!(
        ml <= x + 20 && mt <= y + 20 && ml + 40 >= x.saturating_sub(40),
        "the menu ({ml},{mt}-{mr},{mb}) didn't open at the click ({x},{y})"
    );

    // Click the page well away from the menu.
    let (cx, cy) = (r - 60, b - 60);
    assert!(cx > mr || cy > mb, "no room outside the menu to click");
    a.shim.ok("click", json!({"x": cx, "y": cy}));
    let (closed, shot) = a.shim.screenshot_until(Duration::from_secs(5), |shot| {
        shot.changed_pixels(&page) < 200
    });
    assert!(
        closed,
        "the menu didn't close on a click outside it: {} pixels still differ from the page",
        shot.changed_pixels(&page)
    );
}

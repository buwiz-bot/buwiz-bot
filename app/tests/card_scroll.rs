//! M4 on real AppKit views: Allow's state comes from the text as AppKit lays
//! it out. A pure test can't see this go wrong: a frame AppKit hasn't grown yet
//! reads as a card that fits, and Allow is on over 24 unread lines.

#[cfg(target_os = "macos")]
fn main() {
    use buwiz_bot::agent_hub::card::allow_on;
    use nsh_platform_macos::objc2::MainThreadMarker;

    let mtm = MainThreadMarker::new().expect("on the main thread");
    let lines = |n: usize| (0..n).map(|i| format!("Entry {i:04}, 2026-11-10: ₱{}.00", 100 + i)).collect::<Vec<_>>();
    assert_eq!(allow_on(mtm, &lines(3)), Some((true, true)), "a card that fits has Allow on at once");
    assert_eq!(allow_on(mtm, &lines(24)), Some((false, true)), "a long batch waits for its end");
    assert_eq!(allow_on(mtm, &lines(80)), Some((false, true)), "the longest card");
    println!("test card_scroll ... ok");
}

#[cfg(not(target_os = "macos"))]
fn main() {}

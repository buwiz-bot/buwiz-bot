//! The whole life of a box against the Docker on this machine. Ignored by default because
//! CI (macos-15) has no daemon; run it where there is one:
//!
//!     cd broker && cargo test -p rox-box --test against_a_real_container -- --ignored
//!
//! `ROX_BOX_IMAGE` picks the image (a `grok-box` gets a real screenshot; the default
//! `debian:stable-slim` must answer the screenshot with the typed "no screen" error).
//! Transcribed from opengrok-server `crates/opengrok-box/tests/against_a_real_container.rs`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use rox_box::{BoxError, BoxState, Computer, CreateBox, DockerComputer, Screen};

/// Removes the container (and its volumes) if the test dies halfway.
struct Reaper(String);

impl Drop for Reaper {
    fn drop(&mut self) {
        let _ = std::process::Command::new("docker")
            .args(["rm", "-f", "-v", &self.0])
            .output();
    }
}

#[tokio::test]
#[ignore = "needs a running Docker daemon"]
async fn create_echo_screenshot_stop_destroy() {
    let computer = DockerComputer::from_env();
    if let Err(reason) = computer.availability().await {
        eprintln!("skipping: {}", reason.message());
        return;
    }
    let pulled = computer.pull_latest().await.expect("pull the image");
    assert!(pulled.local, "the image is local after a pull");

    let id = computer
        .create(CreateBox {
            name: Some("rox-box test".into()),
            ..CreateBox::default()
        })
        .await
        .expect("create a box");
    let _reaper = Reaper(id.clone());
    assert_eq!(id.len(), 12, "a short container id");

    let state = computer.state(&id).await.expect("state");
    assert!(
        BoxState::parse(&state).is_running(),
        "fresh box is running, got {state}"
    );

    let echoed = computer
        .run(&id, "echo hello from the box", Some(30))
        .await
        .expect("run echo");
    assert_eq!(echoed.exit_code, 0);
    assert_eq!(echoed.stdout.trim(), "hello from the box");
    assert!(!echoed.timed_out);

    computer
        .write_file(&id, "/workspace/notes/hi.txt", "kept on the volume\n")
        .await
        .expect("write a file");
    assert_eq!(
        computer
            .read_file(&id, "/workspace/notes/hi.txt")
            .await
            .expect("read it back"),
        "kept on the volume\n"
    );

    let failing = computer
        .run(&id, "exit 7", None)
        .await
        .expect("run a failing command");
    assert_eq!(failing.exit_code, 7);

    let url = computer
        .expose_port(&id, 3000)
        .await
        .expect("published port");
    assert!(url.starts_with("http://127.0.0.1:"), "{url}");
    assert!(matches!(
        computer.expose_port(&id, 8791).await,
        Err(BoxError::Refused { status: 409, .. })
    ));

    match computer.screenshot(&id, &Screen::Shared).await {
        Ok(shot) => {
            assert!(shot.width > 0 && shot.height > 0 && shot.png.starts_with(b"\x89PNG"));
            let page = computer
                .screen_url(&id, &Screen::Shared)
                .await
                .expect("screen url");
            assert!(
                page.starts_with("http://127.0.0.1:") && page.contains("/vnc.html?"),
                "{page}"
            );
        }
        Err(BoxError::Refused { status: 409, body }) => {
            assert!(
                !computer.is_desktop(),
                "a desktop image must photograph: {body}"
            );
            assert!(matches!(
                computer.screen_url(&id, &Screen::Shared).await,
                Err(BoxError::Refused { status: 409, .. })
            ));
        }
        Err(other) => panic!("screenshot failed in an untyped way: {other}"),
    }

    let first_generation = computer.generation(&id).await;
    computer.stop(&id).await.expect("stop");
    let asleep = computer.state(&id).await.expect("state after stop");
    assert!(
        matches!(BoxState::parse(&asleep), BoxState::Asleep(_)),
        "stopped box is asleep, got {asleep}"
    );
    assert!(
        computer.generation(&id).await > first_generation,
        "a stop is a new generation"
    );

    computer.wake(&id).await.expect("wake");
    assert!(BoxState::parse(&computer.state(&id).await.expect("state after wake")).is_running());
    assert_eq!(
        computer
            .read_file(&id, "/workspace/notes/hi.txt")
            .await
            .expect("file survives a stop"),
        "kept on the volume\n"
    );

    computer.destroy(&id).await.expect("destroy");
    assert_eq!(
        computer.state(&id).await.expect("state after destroy"),
        "absent"
    );
    assert_eq!(
        computer.run(&id, "true", None).await,
        Err(BoxError::NoSuchBox)
    );
}

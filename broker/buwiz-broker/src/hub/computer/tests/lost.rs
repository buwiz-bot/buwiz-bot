//! A computer found with its record lost and nothing of it found: kept for a person, never deleted
//! by anything automatic, and kept or deleted only when they say.

use super::together::{answer_of, open_req};
use super::*;

/// Ann's computer, its record lost and nothing of it found by a new broker's click: Ann is on a new
/// one, and the old one is lost. (the old id, the broker)
async fn lost_one(b: &Bed) -> (String, Arc<Computers>) {
    let old = awake(b, &b.ann.0).await;
    std::fs::remove_file(Computers::dir(&b.ctx, &old).join("machine.json")).unwrap();
    let fresh = Arc::new(Computers::new(b.mock.clone()));
    fresh.set_sight(std::collections::HashMap::new());
    fresh.click(&b.ctx, &b.ann.0).await.unwrap();
    assert_ne!(seated(&b.ctx, &b.ann.0).unwrap().computer, old);
    assert!(Computers::is_lost(&b.ctx, &old).unwrap());
    (old, fresh)
}

fn deleted_anything(b: &Bed) -> bool {
    b.mock
        .calls()
        .iter()
        .any(|c| c.starts_with("sweep") || c == "destroy")
}

fn docker_computers(computers: &Computers, b: &Bed) -> Vec<Value> {
    let shown = computers.providers_json(&b.ctx);
    let docker = shown["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["kind"] == "docker")
        .unwrap()
        .clone();
    docker["computers"].as_array().unwrap().clone()
}

#[tokio::test]
async fn a_lost_computer_is_listed_with_where_to_look() {
    let b = bed("lost-listed");
    let (old, fresh) = lost_one(&b).await;
    let listed = docker_computers(&fresh, &b);
    let row = listed.iter().find(|c| c["id"] == old.as_str()).unwrap();
    assert_eq!(row["lost"], true, "{row}");
    assert!(row["look"].as_str().unwrap().contains(&old), "{row}");
    assert!(
        row["look"]
            .as_str()
            .unwrap()
            .contains(&format!("label=app.buwiz.computer.id={old}")),
        "Docker's line says what to filter on: {row}"
    );
    assert!(
        row.get("leaving").is_none(),
        "it isn't on its way out: {row}"
    );
    // and nothing can sit at it meanwhile
    assert!(fresh.attach(&b.ctx, &b.bea.0, &old).await.is_err());
}

#[tokio::test]
async fn keep_finds_a_lost_computer_again_and_leaves_it_unseated() {
    let b = bed("lost-keep");
    let (old, fresh) = lost_one(&b).await;
    *b.mock.recoverable.lock().unwrap() = Some(buwiz_computer::Reach::Container {
        name: "mock-found".into(),
    });
    let (_, said) = fresh
        .keep_lost(&b.ctx, ProviderKind::Docker, &old)
        .await
        .unwrap();
    assert!(said.contains("No Bot sits at it"), "{said}");
    assert!(!Computers::is_lost(&b.ctx, &old).unwrap());
    assert!(
        Computers::ids(&b.ctx).contains(&old),
        "a computer again, to join"
    );
    assert!(Computers::seats(&b.ctx, &old).unwrap().is_empty());
    assert!(Computers::dir(&b.ctx, &old).join("machine.json").exists());
    assert!(
        Computers::stop_owed(&b.ctx, &old).exists(),
        "what runs still has the old secrets"
    );
    assert_eq!(
        Computers::reach_of(&b.ctx, &old).unwrap(),
        Some(buwiz_computer::Reach::Container {
            name: "mock-found".into()
        }),
        "its row says where it is now"
    );
    let row = docker_computers(&fresh, &b)
        .into_iter()
        .find(|c| c["id"] == old.as_str())
        .unwrap();
    assert!(row.get("lost").is_none(), "{row}");
    assert!(!deleted_anything(&b), "{:?}", b.mock.calls());
    // a Bot can sit at it again
    fresh.attach(&b.ctx, &b.bea.0, &old).await.unwrap();
}

/// A provider whose recover finds it (as boat's now does): kept, it's never swept however long it
/// sits, and the first wake stops what runs with the old secrets before it starts again.
#[tokio::test]
async fn a_kept_computer_is_never_swept_and_its_first_wake_stops_it_first() {
    let b = bed("lost-keep-wake");
    let (old, fresh) = lost_one(&b).await;
    // Ann's new computer finishes its own wake first, so the calls below are only old's
    for _ in 0..400 {
        if fresh.status(&b.ctx, &b.ann.0).await["state"] == "awake" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    *b.mock.recoverable.lock().unwrap() = Some(buwiz_computer::Reach::Container {
        name: "mock-found".into(),
    });
    fresh
        .keep_lost(&b.ctx, ProviderKind::Docker, &old)
        .await
        .unwrap();
    for _ in 0..3 {
        past_just_gone(&b);
        fresh.sleep_the_idle(&b.ctx).await;
    }
    assert!(!deleted_anything(&b), "{:?}", b.mock.calls());
    assert!(Computers::ids(&b.ctx).contains(&old));
    fresh.attach(&b.ctx, &b.bea.0, &old).await.unwrap();
    b.mock.calls.lock().unwrap().clear();
    fresh.click(&b.ctx, &b.bea.0).await.unwrap();
    for _ in 0..400 {
        if b.mock.calls().contains(&"wake".to_owned()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let calls = b.mock.calls();
    let at = |c: &str| {
        calls
            .iter()
            .position(|x| x == c)
            .unwrap_or_else(|| panic!("no {c}: {calls:?}"))
    };
    assert!(at("sleep Deep") < at("wake"), "{calls:?}");
    assert!(at("sleep Deep") < at("screen_up b_bea"), "{calls:?}");
    assert!(!Computers::stop_owed(&b.ctx, &old).exists());
    assert!(!deleted_anything(&b), "{calls:?}");
}

/// Nothing of it found, and its Bot taken off before the new one is made: nothing is made, since a
/// make can be a billed cloud create.
#[tokio::test]
async fn a_bot_taken_off_before_the_make_makes_nothing() {
    let b = bed("lost-off-before-make");
    let old = awake(&b, &b.ann.0).await;
    std::fs::remove_file(Computers::dir(&b.ctx, &old).join("machine.json")).unwrap();
    let fresh = Arc::new(Computers::new(b.mock.clone()));
    fresh.set_sight(std::collections::HashMap::new());
    fresh.detach(&b.ctx, &b.ann.0).await.unwrap();
    b.mock.calls.lock().unwrap().clear();
    assert_eq!(fresh.found_again(&b.ctx, &b.ann.0, &old).await.unwrap(), None);
    assert!(
        !b.mock.calls().iter().any(|c| c.starts_with("create")),
        "{:?}",
        b.mock.calls()
    );
}

#[tokio::test]
async fn keep_that_still_cant_find_it_changes_nothing_and_deletes_nothing() {
    let b = bed("lost-keep-not-found");
    let (old, fresh) = lost_one(&b).await;
    let refused = fresh
        .keep_lost(&b.ctx, ProviderKind::Docker, &old)
        .await
        .unwrap_err();
    assert_eq!(refused.code(), "still_lost");
    assert!(
        refused.sentence().contains("Nothing was deleted") && refused.sentence().contains(&old),
        "{}",
        refused.sentence()
    );
    assert!(Computers::is_lost(&b.ctx, &old).unwrap());
    assert!(!deleted_anything(&b), "{:?}", b.mock.calls());
    // a computer that isn't lost isn't kept or deleted here
    let new = seated(&b.ctx, &b.ann.0).unwrap().computer;
    assert_eq!(
        fresh
            .keep_lost(&b.ctx, ProviderKind::Docker, &new)
            .await
            .unwrap_err()
            .code(),
        "not_lost"
    );
    assert_eq!(
        fresh
            .delete_lost(&b.ctx, ProviderKind::Docker, &new)
            .await
            .unwrap_err()
            .code(),
        "not_lost"
    );
    assert_eq!(
        fresh
            .delete_lost(&b.ctx, ProviderKind::Docker, "../x")
            .await
            .unwrap_err()
            .code(),
        "not_lost"
    );
}

#[tokio::test]
async fn delete_is_the_only_thing_that_deletes_a_lost_computer() {
    let b = bed("lost-delete");
    let (old, fresh) = lost_one(&b).await;
    for _ in 0..3 {
        past_just_gone(&b);
        fresh.sleep_the_idle(&b.ctx).await;
    }
    assert!(
        !deleted_anything(&b),
        "the timer deleted it: {:?}",
        b.mock.calls()
    );
    fresh
        .delete_lost(&b.ctx, ProviderKind::Docker, &old)
        .await
        .unwrap();
    assert!(
        b.mock.calls().contains(&format!("sweep {old}")),
        "{:?}",
        b.mock.calls()
    );
    assert!(
        !Computers::ids_where(&b.ctx, "1=1").contains(&old),
        "its row went with it"
    );
    assert!(!Computers::dir(&b.ctx, &old).exists());
    // the Bot's new computer is untouched
    assert!(seated(&b.ctx, &b.ann.0).is_some());
}

#[tokio::test]
async fn a_delete_the_platform_refuses_stays_on_its_way_out_for_the_next_look() {
    let b = bed("lost-delete-down");
    let (old, fresh) = lost_one(&b).await;
    past_just_gone(&b);
    // the delete goes through, and the row won't: the delete isn't done
    b.ctx.db.with(|c| c.execute_batch("CREATE TRIGGER keep_rows BEFORE DELETE ON computers BEGIN SELECT RAISE(ABORT, 'disk full'); END;")).unwrap();
    assert!(fresh
        .delete_lost(&b.ctx, ProviderKind::Docker, &old)
        .await
        .is_err());
    assert!(!Computers::is_lost(&b.ctx, &old).unwrap());
    assert!(
        Computers::retired(&b.ctx, &old).unwrap(),
        "a person said delete: the next look goes on with it"
    );
    b.ctx
        .db
        .with(|c| c.execute_batch("DROP TRIGGER keep_rows"))
        .unwrap();
    fresh.sleep_the_idle(&b.ctx).await;
    assert!(!Computers::ids_where(&b.ctx, "1=1").contains(&old));
}

#[tokio::test]
async fn two_clicks_on_a_lost_computer_make_one_new_one_and_hold_no_seats() {
    let b = bed("lost-two-clicks");
    let old = awake(&b, &b.ann.0).await;
    b.computers.attach(&b.ctx, &b.bea.0, &old).await.unwrap();
    std::fs::remove_file(Computers::dir(&b.ctx, &old).join("machine.json")).unwrap();
    let fresh = Arc::new(Computers::new(b.mock.clone()));
    fresh.set_sight(std::collections::HashMap::new());
    b.mock.create_ms.store(100, Ordering::SeqCst);
    b.mock.calls.lock().unwrap().clear();
    let (one, two) = (fresh.clone(), fresh.clone());
    let (c1, c2, ann, bea) = (
        b.ctx.clone(),
        b.ctx.clone(),
        b.ann.0.clone(),
        b.bea.0.clone(),
    );
    let first = tokio::spawn(async move { one.click(&c1, &ann).await });
    let second = tokio::spawn(async move { two.click(&c2, &bea).await });
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert!(
        fresh.seating.try_lock().is_ok(),
        "a make held every Bot's seats"
    );
    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();
    assert_eq!(
        b.mock
            .calls()
            .iter()
            .filter(|c| c.starts_with("create"))
            .count(),
        1,
        "{:?}",
        b.mock.calls()
    );
    let (ann, bea) = (
        seated(&b.ctx, &b.ann.0).unwrap().computer,
        seated(&b.ctx, &b.bea.0).unwrap().computer,
    );
    assert_eq!(ann, bea);
    assert_ne!(ann, old);
    assert!(
        fresh.making.lock().unwrap().is_empty(),
        "nothing is kept once the clicks are done"
    );
}

#[tokio::test]
async fn keep_and_delete_answer_only_the_apps_token_and_say_what_happened() {
    let b = bed("lost-routes");
    let (old, _) = lost_one(&b).await;
    let req = |tail: &str| {
        let mut req = open_req("docker", json!({"id": old}));
        req.url = format!("/v1/host/computer/providers/docker/{tail}");
        req
    };
    for (tail, mut r) in [("keep", req("keep")), ("delete", req("delete"))] {
        r.headers.clear();
        let answer = if tail == "keep" {
            super::keep_lost(b.ctx.clone(), r).await
        } else {
            super::delete_lost(b.ctx.clone(), r).await
        };
        assert_eq!(answer.status(), StatusCode::UNAUTHORIZED);
    }
    let (status, said) = answer_of(super::keep_lost(b.ctx.clone(), req("keep")).await).await;
    assert_eq!(
        (status, said["code"].as_str()),
        (StatusCode::CONFLICT, Some("still_lost")),
        "{said}"
    );
    assert!(
        said["message"]
            .as_str()
            .unwrap()
            .contains("Nothing was deleted"),
        "{said}"
    );
    past_just_gone(&b);
    let (status, said) = answer_of(super::delete_lost(b.ctx.clone(), req("delete")).await).await;
    assert_eq!(status, StatusCode::OK, "{said}");
    assert!(
        said["provider"]["computers"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["id"] != old.as_str()),
        "{said}"
    );
}

/// A person deletes the last Bot at a computer whose record can't be read: it may still be there
/// with their files, so it's kept as lost, for their Keep or Delete, and nothing sweeps it.
#[tokio::test]
async fn deleting_the_last_bot_at_an_unreadable_record_keeps_it_as_lost() {
    let b = bed("lost-bot-deleted");
    let old = awake(&b, &b.ann.0).await;
    std::fs::remove_file(Computers::dir(&b.ctx, &old).join("machine.json")).unwrap();
    let fresh = Arc::new(Computers::new(b.mock.clone()));
    fresh.set_sight(std::collections::HashMap::new());
    b.mock.calls.lock().unwrap().clear();
    fresh.detach(&b.ctx, &b.ann.0).await.unwrap();
    assert!(seated(&b.ctx, &b.ann.0).is_none());
    assert!(Computers::is_lost(&b.ctx, &old).unwrap());
    assert!(!Computers::retired(&b.ctx, &old).unwrap(), "not on its way out");
    assert!(Computers::seats(&b.ctx, &old).unwrap().is_empty());
    for _ in 0..3 {
        past_just_gone(&b);
        fresh.sleep_the_idle(&b.ctx).await;
    }
    assert!(!deleted_anything(&b), "{:?}", b.mock.calls());
    let row = docker_computers(&fresh, &b)
        .into_iter()
        .find(|c| c["id"] == old.as_str())
        .unwrap();
    assert_eq!(row["lost"], true, "{row}");
    // a person's Delete is what deletes it
    fresh
        .delete_lost(&b.ctx, ProviderKind::Docker, &old)
        .await
        .unwrap();
    assert!(
        b.mock.calls().contains(&format!("sweep {old}")),
        "{:?}",
        b.mock.calls()
    );
    assert!(!Computers::ids_where(&b.ctx, "1=1").contains(&old));
}

/// One whose record reads goes with its last Bot, as a person's delete says.
#[tokio::test]
async fn deleting_the_last_bot_at_a_readable_record_still_deletes_it() {
    let b = bed("lost-bot-deleted-readable");
    let old = awake(&b, &b.ann.0).await;
    b.computers.detach(&b.ctx, &b.ann.0).await.unwrap();
    assert!(!Computers::is_lost(&b.ctx, &old).unwrap());
    assert!(Computers::retired(&b.ctx, &old).unwrap());
    b.computers.sleep_the_idle(&b.ctx).await;
    assert!(
        b.mock.calls().contains(&"destroy".to_owned()),
        "{:?}",
        b.mock.calls()
    );
    assert!(!Computers::ids_where(&b.ctx, "1=1").contains(&old));
}

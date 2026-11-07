use aivory_protocol::demo::Demo;
use aivory_protocol::{ActivitySample, Autopilot, Device, InboxItem, Settings, TaskColumn};
use serde_json::json;

use super::*;
use crate::testutil::task;

fn temp_db() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("aivory").join("state.db");
    (dir, path)
}

fn table_names(conn: &Connection) -> Vec<String> {
    let mut stmt = conn
        .prepare(
            "SELECT name FROM sqlite_master
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .unwrap();
    stmt.query_map([], |r| r.get(0))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap()
}

fn defaults() -> Settings {
    Demo::new().settings.settings
}

fn answered(id: &str, result: &str) -> InboxItem {
    let mut item = Demo::new().inbox.items.remove(0);
    item.id = id.into();
    item.result = Some(result.into());
    item
}

// ---- migrations ------------------------------------------------------------------------

#[test]
fn migrates_an_empty_file_to_the_latest_version() {
    let (_dir, path) = temp_db();
    let store = Store::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(store.journal_mode().unwrap(), "wal");
    drop(store);

    let raw = Connection::open(&path).unwrap();
    assert_eq!(
        table_names(&raw),
        [
            "activity_samples",
            "devices",
            "inbox_results",
            "schema_version",
            "settings",
            "tasks"
        ]
    );
    let rows: u32 = raw
        .query_row("SELECT COUNT(*) FROM schema_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 1, "schema_version holds one row");
}

#[test]
fn migrates_a_file_at_version_1_and_keeps_its_data() {
    let (_dir, path) = temp_db();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    {
        // A file as the first release wrote it: version 1, one task, no devices table.
        let mut conn = Connection::open(&path).unwrap();
        migrate(&mut conn, 1).unwrap();
        assert_eq!(read_version(&conn).unwrap(), 1);
        assert!(!table_names(&conn).contains(&"devices".to_string()));
        let t = task("old", "web");
        conn.execute(
            "INSERT INTO tasks (id, board_column, codebase, started, body)
             VALUES (?1, 'planned', 'web', NULL, ?2)",
            params![t.id, serde_json::to_string(&t).unwrap()],
        )
        .unwrap();
    }
    let mut store = Store::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(store.task("old").unwrap(), Some(task("old", "web")));
    // The new table works.
    let device = Demo::new().settings.devices.remove(0);
    store.put_device(&device).unwrap();
    assert_eq!(store.device(&device.id).unwrap(), Some(device));
}

#[test]
fn migrating_twice_changes_nothing() {
    let (_dir, path) = temp_db();
    let mut first = Store::open(&path).unwrap();
    first.put_task(&task("t1", "web")).unwrap();
    drop(first);
    let second = Store::open(&path).unwrap();
    assert_eq!(second.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(second.tasks().unwrap().len(), 1);
}

#[test]
fn refuses_a_file_from_a_newer_build() {
    let (_dir, path) = temp_db();
    drop(Store::open(&path).unwrap());
    Connection::open(&path)
        .unwrap()
        .execute("UPDATE schema_version SET version = 99", [])
        .unwrap();
    match Store::open(&path) {
        Err(StoreError::TooNew { found: 99, .. }) => {}
        other => panic!("expected TooNew, got {other:?}"),
    }
}

#[test]
fn a_failed_migration_rolls_back_whole() {
    let mut conn = Connection::open_in_memory().unwrap();
    migrate(&mut conn, 1).unwrap();
    // Break migration 2 by taking its table name first.
    conn.execute_batch("CREATE TABLE devices (x)").unwrap();
    assert!(migrate(&mut conn, 2).is_err());
    assert_eq!(read_version(&conn).unwrap(), 1);
}

#[test]
fn open_creates_the_parent_directory() {
    let (_dir, path) = temp_db();
    assert!(!path.parent().unwrap().exists());
    Store::open(&path).unwrap();
    assert!(path.exists());
}

#[test]
fn in_memory_store_is_migrated() {
    let store = Store::open_in_memory().unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    assert!(store.tasks().unwrap().is_empty());
}

// ---- one writer, many readers ----------------------------------------------------------

#[test]
fn read_only_handle_reads_and_cannot_write() {
    let (_dir, path) = temp_db();
    let mut writer = Store::open(&path).unwrap();
    writer.put_task(&task("t1", "web")).unwrap();

    let mut reader = Store::open_read_only(&path).unwrap();
    assert_eq!(reader.tasks().unwrap().len(), 1);
    assert!(matches!(
        reader.put_task(&task("t2", "web")),
        Err(StoreError::Sqlite(_))
    ));

    // The writer goes on, and the reader sees the commit.
    writer.put_task(&task("t2", "web")).unwrap();
    assert_eq!(reader.tasks().unwrap().len(), 2);
}

#[test]
fn read_only_handle_refuses_an_old_file() {
    let (_dir, path) = temp_db();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut conn = Connection::open(&path).unwrap();
    migrate(&mut conn, 1).unwrap();
    drop(conn);
    assert!(matches!(
        Store::open_read_only(&path),
        Err(StoreError::TooOld { found: 1, .. })
    ));
}

// ---- tasks -----------------------------------------------------------------------------

#[test]
fn tasks_round_trip_in_creation_order() {
    let mut store = Store::open_in_memory().unwrap();
    let all = Demo::new().tasks;
    assert!(!all.is_empty());
    for t in &all {
        store.put_task(t).unwrap();
    }
    assert_eq!(store.tasks().unwrap(), all);
    for t in &all {
        assert_eq!(store.task(&t.id).unwrap().as_ref(), Some(t));
    }
    assert_eq!(store.task("missing").unwrap(), None);
}

#[test]
fn replacing_a_task_keeps_its_place_and_changes_the_body() {
    let mut store = Store::open_in_memory().unwrap();
    for id in ["a", "b", "c"] {
        store.put_task(&task(id, "web")).unwrap();
    }
    let mut b = task("b", "api");
    b.column = TaskColumn::Working;
    b.started = Some(5);
    b.cost = 1.25;
    store.put_task(&b).unwrap();
    let ids: Vec<_> = store.tasks().unwrap().into_iter().map(|t| t.id).collect();
    assert_eq!(ids, ["a", "b", "c"]);
    assert_eq!(store.task("b").unwrap(), Some(b));
}

#[test]
fn queued_tasks_are_planned_and_not_started_in_creation_order() {
    let mut store = Store::open_in_memory().unwrap();
    let mut started = task("started", "web");
    started.started = Some(1);
    let mut working = task("working", "web");
    working.column = TaskColumn::Working;
    for t in [task("q1", "web"), started, working, task("q2", "api")] {
        store.put_task(&t).unwrap();
    }
    let ids: Vec<_> = store
        .queued_tasks()
        .unwrap()
        .into_iter()
        .map(|t| t.id)
        .collect();
    assert_eq!(ids, ["q1", "q2"]);
    // A task that starts leaves the queue; a later task keeps its age slot.
    let mut q1 = task("q1", "web");
    q1.column = TaskColumn::Working;
    q1.started = Some(9);
    store.put_task(&q1).unwrap();
    assert_eq!(store.queued_tasks().unwrap().len(), 1);
}

#[test]
fn delete_task_reports_whether_a_row_went() {
    let mut store = Store::open_in_memory().unwrap();
    store.put_task(&task("a", "web")).unwrap();
    assert!(store.delete_task("a").unwrap());
    assert!(!store.delete_task("a").unwrap());
    assert!(store.tasks().unwrap().is_empty());
    // A new task after a delete is newer than every other task.
    store.put_task(&task("b", "web")).unwrap();
    store.put_task(&task("c", "web")).unwrap();
    store.delete_task("b").unwrap();
    store.put_task(&task("b", "web")).unwrap();
    let ids: Vec<_> = store.tasks().unwrap().into_iter().map(|t| t.id).collect();
    assert_eq!(ids, ["c", "b"]);
}

// ---- inbox results ---------------------------------------------------------------------

#[test]
fn inbox_result_round_trip() {
    let mut store = Store::open_in_memory().unwrap();
    let item = answered("i1", "allowed");
    store.put_inbox_result(&item, 1_000).unwrap();
    let got = store.inbox_result("i1").unwrap().unwrap();
    assert_eq!(got.item, item);
    assert_eq!(got.answered, 1_000);
    assert_eq!(store.inbox_result("nope").unwrap(), None);
}

#[test]
fn demo_done_items_round_trip() {
    let mut store = Store::open_in_memory().unwrap();
    let done = Demo::new().inbox.done;
    for (n, item) in done.iter().enumerate() {
        store.put_inbox_result(item, n as i64).unwrap();
    }
    let got: Vec<_> = store
        .inbox_results(100)
        .unwrap()
        .into_iter()
        .map(|a| a.item)
        .collect();
    // Newest answer first.
    let mut want = done.clone();
    want.reverse();
    assert_eq!(got, want);
}

#[test]
fn inbox_item_without_a_result_is_refused() {
    let mut store = Store::open_in_memory().unwrap();
    let mut item = answered("i1", "x");
    item.result = None;
    assert!(matches!(
        store.put_inbox_result(&item, 1),
        Err(StoreError::Invalid(_))
    ));
    assert!(store.inbox_results(10).unwrap().is_empty());
}

#[test]
fn inbox_results_newest_first_with_limit_replace_prune_delete() {
    let mut store = Store::open_in_memory().unwrap();
    store
        .put_inbox_result(&answered("a", "allowed"), 100)
        .unwrap();
    store
        .put_inbox_result(&answered("b", "denied"), 300)
        .unwrap();
    store
        .put_inbox_result(&answered("c", "approved"), 200)
        .unwrap();
    let ids = |s: &Store, n| -> Vec<String> {
        s.inbox_results(n)
            .unwrap()
            .into_iter()
            .map(|a| a.item.id)
            .collect()
    };
    assert_eq!(ids(&store, 10), ["b", "c", "a"]);
    assert_eq!(ids(&store, 2), ["b", "c"]);

    // Answer "a" again: the row is replaced, not doubled.
    store
        .put_inbox_result(&answered("a", "merged"), 400)
        .unwrap();
    assert_eq!(ids(&store, 10), ["a", "b", "c"]);
    let a = store.inbox_result("a").unwrap().unwrap();
    assert_eq!(
        (a.item.result.as_deref(), a.answered),
        (Some("merged"), 400)
    );

    assert_eq!(store.prune_inbox_results(300).unwrap(), 1); // "c" at 200
    assert_eq!(ids(&store, 10), ["a", "b"]);
    assert!(store.delete_inbox_result("b").unwrap());
    assert!(!store.delete_inbox_result("b").unwrap());
    assert_eq!(ids(&store, 10), ["a"]);
}

// ---- settings --------------------------------------------------------------------------

#[test]
fn empty_store_returns_the_defaults() {
    let store = Store::open_in_memory().unwrap();
    assert_eq!(store.settings(&defaults()).unwrap(), defaults());
    assert_eq!(store.autopilot().unwrap(), Autopilot::default());
}

#[test]
fn settings_round_trip() {
    let mut store = Store::open_in_memory().unwrap();
    let mut s = defaults();
    s.autopilot.on = false;
    s.autopilot.max_running = 7;
    s.quiet_hours.from = "21:30".into();
    s.providers.get_mut("claude").unwrap().enabled = false;
    store.save_settings(&s).unwrap();
    assert_eq!(store.settings(&defaults()).unwrap(), s);
    // One row per top-level key of Settings.
    for key in ["providers", "autopilot", "display", "notify", "quietHours"] {
        assert!(store.setting(key).unwrap().is_some(), "{key}");
    }
    assert_eq!(store.autopilot().unwrap(), s.autopilot);
}

#[test]
fn raw_setting_set_get_replace_delete() {
    let mut store = Store::open_in_memory().unwrap();
    assert_eq!(store.setting("k").unwrap(), None);
    store.set_setting("k", &json!({"a": [1, 2]})).unwrap();
    assert_eq!(store.setting("k").unwrap(), Some(json!({"a": [1, 2]})));
    store.set_setting("k", &json!(true)).unwrap();
    assert_eq!(store.setting("k").unwrap(), Some(json!(true)));
    assert!(store.delete_setting("k").unwrap());
    assert!(!store.delete_setting("k").unwrap());
    assert_eq!(store.setting("k").unwrap(), None);
}

#[test]
fn a_field_that_a_newer_build_added_comes_from_the_defaults() {
    let mut store = Store::open_in_memory().unwrap();
    // An older build saved the autopilot rules without `order`.
    store
        .set_setting(
            "autopilot",
            &json!({"on": false, "parkAfterMin": 30, "maxRunning": 3,
                    "maxPerCodebase": 2, "startQueued": false}),
        )
        .unwrap();
    let ap = store.autopilot().unwrap();
    assert_eq!(
        ap,
        Autopilot {
            on: false,
            park_after_min: 30,
            max_running: 3,
            max_per_codebase: 2,
            start_queued: false,
            order: Autopilot::default().order,
        }
    );
    let all = store.settings(&defaults()).unwrap();
    assert_eq!(all.autopilot, ap);
    assert_eq!(all.display, defaults().display);
}

#[test]
fn apply_setting_validates_clamps_and_saves() {
    let mut store = Store::open_in_memory().unwrap();
    let next = store
        .apply_setting(&defaults(), "autopilot.parkAfterMin", json!(47))
        .unwrap();
    assert_eq!(next.autopilot.park_after_min, 45, "snaps to the step of 5");
    assert_eq!(store.autopilot().unwrap().park_after_min, 45);

    let next = store
        .apply_setting(&defaults(), "autopilot.maxRunning", json!(1000))
        .unwrap();
    assert_eq!(next.autopilot.max_running, 64, "clamped to the range");
    // The first change survives the second.
    assert_eq!(next.autopilot.park_after_min, 45);

    let before = store.settings(&defaults()).unwrap();
    for (key, value) in [("autopilot.nope", json!(1)), ("autopilot.on", json!("yes"))] {
        assert!(matches!(
            store.apply_setting(&defaults(), key, value),
            Err(StoreError::Invalid(_))
        ));
    }
    assert_eq!(store.settings(&defaults()).unwrap(), before);
}

// ---- activity samples ------------------------------------------------------------------

fn sample(at: UnixMs, active: u32) -> ActivitySample {
    ActivitySample {
        at,
        active,
        waiting: 2,
        idle: 3,
    }
}

#[test]
fn activity_round_trip_replace_since_prune() {
    let mut store = Store::open_in_memory().unwrap();
    for at in [300, 100, 200] {
        store.put_activity(&sample(at, 1)).unwrap();
    }
    assert_eq!(
        store.activity_since(0).unwrap(),
        [sample(100, 1), sample(200, 1), sample(300, 1)]
    );
    // The same minute replaces the row.
    store.put_activity(&sample(200, 9)).unwrap();
    assert_eq!(
        store.activity_since(100).unwrap(),
        [sample(200, 9), sample(300, 1)],
        "since is exclusive"
    );
    assert_eq!(store.prune_activity(200).unwrap(), 2);
    assert_eq!(store.activity_since(0).unwrap(), [sample(300, 1)]);
}

#[test]
fn activity_holds_large_counts() {
    let mut store = Store::open_in_memory().unwrap();
    let s = ActivitySample {
        at: 1_800_000_000_000,
        active: u32::MAX,
        waiting: 0,
        idle: 0,
    };
    store.put_activity(&s).unwrap();
    assert_eq!(store.activity_since(0).unwrap(), [s]);
}

// ---- devices ---------------------------------------------------------------------------

#[test]
fn devices_round_trip() {
    let mut store = Store::open_in_memory().unwrap();
    let devices = Demo::new().settings.devices;
    assert!(devices.len() >= 2);
    for d in devices.iter().rev() {
        store.put_device(d).unwrap();
    }
    // Oldest pairing first.
    let mut want = devices.clone();
    want.sort_by_key(|d| d.paired);
    assert_eq!(store.devices().unwrap(), want);
    assert_eq!(
        store.device(&devices[0].id).unwrap().as_ref(),
        Some(&devices[0])
    );
    assert_eq!(store.device("none").unwrap(), None);
}

#[test]
fn device_replace_and_delete() {
    let mut store = Store::open_in_memory().unwrap();
    let mut d = Device {
        id: "d1".into(),
        name: "Pixel".into(),
        os: "Android 16".into(),
        paired: 10,
        last_seen: 20,
        online: true,
    };
    store.put_device(&d).unwrap();
    d.online = false;
    d.last_seen = 99;
    d.name = "Pixel 9".into();
    store.put_device(&d).unwrap();
    assert_eq!(store.devices().unwrap(), [d.clone()]);
    assert!(store.delete_device("d1").unwrap());
    assert!(!store.delete_device("d1").unwrap());
    assert!(store.devices().unwrap().is_empty());
}

// ---- durability ------------------------------------------------------------------------

#[test]
fn everything_survives_a_reopen() {
    let (_dir, path) = temp_db();
    let demo = Demo::new();
    {
        let mut store = Store::open(&path).unwrap();
        for t in &demo.tasks {
            store.put_task(t).unwrap();
        }
        store
            .put_inbox_result(&answered("i1", "allowed"), 5)
            .unwrap();
        store.save_settings(&demo.settings.settings).unwrap();
        store.put_activity(&sample(60_000, 4)).unwrap();
        store.put_device(&demo.settings.devices[0]).unwrap();
    }
    let store = Store::open(&path).unwrap();
    assert_eq!(store.tasks().unwrap(), demo.tasks);
    assert_eq!(store.inbox_result("i1").unwrap().unwrap().answered, 5);
    assert_eq!(store.settings(&defaults()).unwrap(), demo.settings.settings);
    assert_eq!(store.activity_since(0).unwrap(), [sample(60_000, 4)]);
    assert_eq!(store.devices().unwrap(), [demo.settings.devices[0].clone()]);
}

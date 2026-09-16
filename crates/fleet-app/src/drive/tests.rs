use super::*;
use fleet_drive::protocol::{
    AdvanceArgs, AssertArgs, AwaitArgs, DumpArgs, EmptyArgs, KeyArgs, ShotArgs,
};
use fleet_ui_kit::{Icon, Toast};
use gpui::{Context, IntoElement, Render, TestAppContext, div};
use std::os::unix::net::UnixStream;

/// The toast dwell these tests advance past, chosen so real waiting would be obvious.
const TEST_DWELL_MS: u64 = 4_000;
const TEST_DWELL: Duration = Duration::from_millis(TEST_DWELL_MS);

struct DriverWindow;
impl Render for DriverWindow {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// A window plus the one application state its driver projects, as `spawn` pairs them.
fn driver(cx: &mut TestAppContext) -> (Entity<AppState>, AsyncWindowContext) {
    let window = cx.add_window(|_, _| DriverWindow);
    let state = cx.update(|cx| {
        cx.new(|_| {
            AppState::new(
                std::path::PathBuf::from("/tmp/fleet-harness-test"),
                Instant::now(),
            )
        })
    });
    let async_cx = window
        .update(cx, |_, window, cx| window.to_async(cx))
        .expect("window context");
    (state, async_cx)
}

fn harness_for(socket: Option<PathBuf>) -> Harness {
    Harness {
        run_id: "test".to_owned(),
        deterministic: true,
        window_size: parse_size(None),
        title: SharedString::new_static("Fleet [harness:test]"),
        lane: "headless".to_owned(),
        headless: true,
        socket,
    }
}

fn socket_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "fleet-app-drive-{name}-{}-{:?}.sock",
        std::process::id(),
        std::thread::current().id()
    ))
}

#[test]
fn harness_geometry_is_identical_across_launches() {
    // Two launches of the same run read the same environment, so the pinned size must be a
    // pure function of it; a malformed value falls back rather than varying.
    assert_eq!(parse_size(None), parse_size(None));
    assert_eq!(parse_size(None), size(px(1440.0), px(900.0)));
    assert_eq!(parse_size(Some("1600x1000")), size(px(1600.0), px(1000.0)));
    assert_eq!(parse_size(Some("1600X1000")), size(px(1600.0), px(1000.0)));
    for bad in [
        "",
        "1600",
        "widexhigh",
        "0x0",
        "-4x-4",
        "NaNxNaN",
        "infxinf",
    ] {
        assert_eq!(parse_size(Some(bad)), parse_size(None), "{bad:?}");
    }
}

#[test]
fn the_run_id_names_the_window_for_the_compositor() {
    let id = run_id(Some(std::path::Path::new("/tmp/fleet-harness/abc123.sock")));
    assert_eq!(id, "abc123");
    assert_eq!(format!("Fleet [harness:{id}]"), "Fleet [harness:abc123]");
    assert!(!run_id(None).is_empty());
}

#[test]
fn settling_and_link_opening_are_named_as_sole_idle_blockers() {
    for (counter, value) in [
        ("settling_mutations", json!(1)),
        ("link_opening", json!(true)),
    ] {
        let mut idle = json!({
            "in_flight_requests": 0,
            "settling_mutations": 0,
            "running_jobs": 0,
            "pending_frame": false,
            "live_toast_timers": 0,
            "armed_debounces": 0,
            "link_opening": false,
        });
        idle[counter] = value.clone();
        assert_eq!(busy_counters(&idle), format!("{counter}={value}"));
    }
}

#[test]
fn docs_pin_the_headless_window_frame_restriction() {
    let docs = include_str!("../../../../docs/TESTING-HARNESS.md");
    assert!(
        docs.contains("A headless `await` does not repaint, so `window.frame` freezes"),
        "keep the documented headless frame-freezing restriction"
    );
}

#[gpui::test]
async fn an_unknown_command_fails_without_closing_the_connection(cx: &mut TestAppContext) {
    let harness = harness_for(None);
    let (state, mut async_cx) = driver(cx);

    let request = Request {
        id: 9,
        cmd: "future".to_owned(),
        args: json!({}),
    };
    let (response, quit) = answer(&harness, &state, &request, &mut async_cx).await;
    assert_eq!(response.id, 9);
    assert!(!response.ok && !quit);
    assert!(
        response
            .error
            .as_deref()
            .is_some_and(|error| error.contains("unknown command \"future\"")),
        "{:?}",
        response.error
    );
}

#[gpui::test]
async fn dump_answers_the_whole_snapshot_and_its_summary(cx: &mut TestAppContext) {
    let harness = harness_for(None);
    let (state, mut async_cx) = driver(cx);

    let dump = Request::new(
        1,
        &Command::Dump(DumpArgs {
            name: "hub".to_owned(),
        }),
    )
    .expect("encode dump");
    let (response, _) = answer(&harness, &state, &dump, &mut async_cx).await;
    assert!(response.ok, "{:?}", response.error);
    assert_eq!(response.data["name"], json!("hub"));
    assert_eq!(
        response.data["snapshot"]["version"],
        json!(crate::state::SNAPSHOT_VERSION)
    );
    assert_eq!(response.data["snapshot"]["screen"], json!("Hub"));
    // The pinned title and geometry reach the snapshot through the same update path.
    assert_eq!(
        response.data["snapshot"]["window"]["title"],
        json!("Fleet [harness:test]")
    );
    assert!(
        response.data["summary"]
            .as_str()
            .is_some_and(|summary| summary.starts_with("screen=Hub ")),
        "{:?}",
        response.data["summary"]
    );
    // Two dumps with nothing between them describe the same revision.
    let (again, _) = answer(&harness, &state, &dump, &mut async_cx).await;
    assert_eq!(response.data["revision"], again.data["revision"]);
}

#[gpui::test]
async fn assert_reports_the_actual_value_it_saw(cx: &mut TestAppContext) {
    let harness = harness_for(None);
    let (state, mut async_cx) = driver(cx);

    let held = Request::new(
        1,
        &Command::Assert(AssertArgs {
            predicate: "screen == Hub".to_owned(),
        }),
    )
    .expect("encode assert");
    let (response, _) = answer(&harness, &state, &held, &mut async_cx).await;
    assert!(response.ok, "{:?}", response.error);
    assert_eq!(response.data["satisfied"], json!(true));
    assert!(
        response.data["snapshot"].is_null(),
        "a held assert carries no dump"
    );

    let failed = Request::new(
        2,
        &Command::Assert(AssertArgs {
            predicate: "screen == Workspace".to_owned(),
        }),
    )
    .expect("encode assert");
    let (response, _) = answer(&harness, &state, &failed, &mut async_cx).await;
    assert!(!response.ok);
    assert_eq!(response.data["clauses"][0]["actual"], json!("Hub"));
    assert_eq!(response.data["snapshot"]["screen"], json!("Hub"));
    assert!(
        response
            .error
            .as_deref()
            .is_some_and(|error| error.contains("screen is \"Hub\"")),
        "{:?}",
        response.error
    );

    // A malformed predicate is a command error, not a verdict.
    let malformed = Request::new(
        3,
        &Command::Assert(AssertArgs {
            predicate: "screen ==".to_owned(),
        }),
    )
    .expect("encode assert");
    let (response, _) = answer(&harness, &state, &malformed, &mut async_cx).await;
    assert!(!response.ok);
    assert!(response.data["clauses"].is_null());
}

#[gpui::test]
async fn await_returns_as_soon_as_its_predicate_holds_and_times_out_otherwise(
    cx: &mut TestAppContext,
) {
    let harness = harness_for(None);
    let (state, mut async_cx) = driver(cx);

    let satisfied = Request::new(
        1,
        &Command::Await(AwaitArgs {
            predicate: "screen == Hub && overlay absent".to_owned(),
            timeout_ms: 5_000,
        }),
    )
    .expect("encode await");
    let (response, _) = answer(&harness, &state, &satisfied, &mut async_cx).await;
    assert!(response.ok, "{:?}", response.error);
    assert_eq!(response.data["satisfied"], json!(true));

    let never = Request::new(
        2,
        &Command::Await(AwaitArgs {
            predicate: "screen == Workspace".to_owned(),
            timeout_ms: 20,
        }),
    )
    .expect("encode await");
    let (response, _) = answer(&harness, &state, &never, &mut async_cx).await;
    assert!(!response.ok);
    // The timeout names the clause, the value it saw, and all six idle fields.
    assert!(
        response
            .error
            .as_deref()
            .is_some_and(|error| error.contains("timed out after 20 ms")),
        "{:?}",
        response.error
    );
    assert_eq!(response.data["clauses"][0]["actual"], json!("Hub"));
    assert_eq!(response.data["idle"]["running_jobs"], json!(0));
    assert!(response.data["idle"]["idle"].is_boolean());
}

/// Pushes one toast onto the real state, exactly as an update path would.
fn toast(cx: &mut TestAppContext, state: &Entity<AppState>, text: &'static str) {
    cx.update(|cx| {
        state.update(cx, |state, cx| {
            state.toast(
                Toast::new(text).icon(Icon::Info),
                Instant::now(),
                TEST_DWELL,
            );
            cx.notify();
        });
    });
}

#[gpui::test]
async fn await_answers_on_the_state_change_that_satisfies_it(cx: &mut TestAppContext) {
    let harness = harness_for(None);
    let (state, mut async_cx) = driver(cx);

    // The change lands on a timer, so the waiter is already parked on its observation when
    // the state moves. Nothing polls: the only wake-up is the `cx.notify()` below.
    let updater = cx.update(|cx| {
        let state = state.clone();
        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(Duration::from_millis(5))
                .await;
            state.update(cx, |state, cx| {
                state.toast(
                    Toast::new("harness-wake").icon(Icon::Info),
                    Instant::now(),
                    TEST_DWELL,
                );
                cx.notify();
            });
        })
    });

    let started = Instant::now();
    let request = Request::new(
        1,
        &Command::Await(AwaitArgs {
            predicate: "toasts[0].text == \"harness-wake\"".to_owned(),
            timeout_ms: 60_000,
        }),
    )
    .expect("encode await");
    let (response, _) = answer(&harness, &state, &request, &mut async_cx).await;
    updater.await;
    assert!(response.ok, "{:?}", response.error);
    assert_eq!(response.data["satisfied"], json!(true));
    assert!(
        response.data["snapshot"].is_null(),
        "a satisfied await carries no dump"
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the waiter answered on the change, not on its 60 s timeout"
    );
}

#[gpui::test]
async fn an_await_timeout_names_the_idle_counter_that_is_still_busy(cx: &mut TestAppContext) {
    let harness = harness_for(None);
    let (state, mut async_cx) = driver(cx);
    toast(cx, &state, "harness-busy");

    let request = Request::new(
        1,
        &Command::Await(AwaitArgs {
            predicate: "idle".to_owned(),
            timeout_ms: 20,
        }),
    )
    .expect("encode await");
    let (response, _) = answer(&harness, &state, &request, &mut async_cx).await;
    assert!(!response.ok);
    let error = response.error.unwrap_or_default();
    assert!(
        error.contains("still busy: live_toast_timers=1"),
        "an `idle` timeout names the constituent, not just the object: {error}"
    );
    assert_eq!(response.data["idle"]["live_toast_timers"], json!(1));
    assert_eq!(response.data["idle"]["idle"], json!(false));
    // The last snapshot rides along so the failure explains itself.
    assert_eq!(
        response.data["snapshot"]["toasts"][0]["text"],
        json!("harness-busy")
    );
}

#[gpui::test]
async fn advance_expires_a_toast_without_waiting_out_its_dwell(cx: &mut TestAppContext) {
    let harness = harness_for(None);
    let (state, mut async_cx) = driver(cx);
    toast(cx, &state, "harness-dwell");

    let present = Request::new(
        1,
        &Command::Assert(AssertArgs {
            predicate: "toasts[0].text == \"harness-dwell\"".to_owned(),
        }),
    )
    .expect("encode assert");
    let (response, _) = answer(&harness, &state, &present, &mut async_cx).await;
    assert!(response.ok, "{:?}", response.error);

    let started = Instant::now();
    let advance = Request::new(
        2,
        &Command::Advance(AdvanceArgs {
            millis: TEST_DWELL_MS + 1,
        }),
    )
    .expect("encode advance");
    let (response, _) = answer(&harness, &state, &advance, &mut async_cx).await;
    assert!(response.ok, "{:?}", response.error);
    assert_eq!(response.data["changed"], json!(true));

    let gone = Request::new(
        3,
        &Command::Assert(AssertArgs {
            predicate: "toasts[0] absent".to_owned(),
        }),
    )
    .expect("encode assert");
    let (response, _) = answer(&harness, &state, &gone, &mut async_cx).await;
    assert!(response.ok, "{:?}", response.error);
    assert!(
        started.elapsed() < TEST_DWELL,
        "the dwell was advanced through, not waited out"
    );
}

#[gpui::test]
async fn meta_reports_the_pinned_window_identity(cx: &mut TestAppContext) {
    let harness = harness_for(None);
    let (state, mut async_cx) = driver(cx);

    let meta = Request::new(1, &Command::Meta(EmptyArgs {})).expect("encode meta");
    let (first, _) = answer(&harness, &state, &meta, &mut async_cx).await;
    assert!(first.ok, "{:?}", first.error);
    assert_eq!(first.data["run_id"], json!("test"));
    assert_eq!(first.data["lane"], json!("headless"));
    assert_eq!(first.data["title"], json!("Fleet [harness:test]"));

    let (second, _) = answer(&harness, &state, &meta, &mut async_cx).await;
    assert_eq!(first.data["bounds"], second.data["bounds"]);
    assert_eq!(first.data["scale_factor"], second.data["scale_factor"]);
}

#[gpui::test]
async fn a_headless_shot_returns_geometry_for_the_runner_to_classify(cx: &mut TestAppContext) {
    let harness = harness_for(None);
    let (state, mut async_cx) = driver(cx);
    let shot = Request::new(
        1,
        &Command::Shot(ShotArgs {
            name: "headless-update".to_owned(),
        }),
    )
    .expect("encode shot");

    let (response, _) = answer(&harness, &state, &shot, &mut async_cx).await;

    assert!(response.ok, "{:?}", response.error);
    assert_eq!(response.data["name"], json!("headless-update"));
    assert_eq!(response.data["lane"], json!("headless"));
}

#[gpui::test]
async fn a_key_is_applied_and_its_handling_reported(cx: &mut TestAppContext) {
    let harness = harness_for(None);
    let (state, mut async_cx) = driver(cx);

    let key = Request::new(
        1,
        &Command::Key(KeyArgs {
            keys: vec!["?".to_owned()],
        }),
    )
    .expect("encode key");
    let (response, _) = answer(&harness, &state, &key, &mut async_cx).await;
    assert!(response.ok, "{:?}", response.error);
    // Nothing in this bare window binds `?`, which is exactly what the runner is told.
    assert_eq!(response.data["handled"], json!([false]));

    let bad = Request::new(
        2,
        &Command::Key(KeyArgs {
            keys: vec!["not-a-key".to_owned()],
        }),
    )
    .expect("encode key");
    let (response, _) = answer(&harness, &state, &bad, &mut async_cx).await;
    assert!(!response.ok);
}

#[gpui::test]
async fn quitting_closes_the_window_stops_the_driver_and_unlinks_its_socket(
    cx: &mut TestAppContext,
) {
    let path = socket_path("close");
    let window = cx.add_window(|_, _| DriverWindow);
    let state = cx.update(|cx| {
        cx.new(|_| {
            AppState::new(
                std::path::PathBuf::from("/tmp/fleet-harness-test"),
                Instant::now(),
            )
        })
    });
    let task = window
        .update(cx, |_, window, cx| {
            spawn(harness_for(Some(path.clone())), state.clone(), window, cx)
        })
        .expect("update the window")
        .expect("a socket was configured");
    cx.run_until_parked();
    assert!(path.exists(), "the driver binds before it serves");

    let mut async_cx = window
        .update(cx, |_, window, cx| window.to_async(cx))
        .expect("window context");
    close_harness_window(&mut async_cx, true).expect("quit closes the window");
    task.await;
    assert!(!path.exists(), "the driver unlinks its socket on shutdown");
    assert!(
        UnixStream::connect(&path).is_err(),
        "a closed window leaves nothing listening"
    );
}

#[gpui::test]
fn virtual_quit_paints_an_unfocused_frame_before_closing(cx: &mut TestAppContext) {
    let window = cx.add_window(|_, _| DriverWindow);
    let mut async_cx = window
        .update(cx, |_, window, cx| window.to_async(cx))
        .expect("window context");

    close_harness_window(&mut async_cx, false).expect("schedule virtual quit");
    let callbacks = window
        .update(cx, |_, window, cx| window.simulate_next_frame(cx))
        .expect("paint the cleanup frame");

    assert_eq!(callbacks, 1, "quit closes from the cleanup frame callback");
}

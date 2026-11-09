/// Anything that changes much of the window moves there. Measured from the pixels of every act
/// of every scene: the frame after the act must not already look like where it ends up, the
/// way a card or a dialog thrown onto the screen does.
#[test]
fn nothing_big_appears_at_once() {
    /// The share of the window an act must change to count.
    const BIG: f64 = 0.04;
    /// How much of its change may be there in the first frame.
    const ARRIVED: f64 = 0.75;
    let _alone = TIMED_SHELLS
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let threads = workers();
    let mut all: Vec<Pop> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|first| {
                scope.spawn(move || {
                    let mut text = None;
                    let mut pops = Vec::new();
                    for scene in SCENES.iter().chain(FILMS).skip(first).step_by(threads) {
                        pops.extend(pops_of(scene, &mut text));
                    }
                    pops
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().unwrap())
            .collect()
    });
    all.sort_by(|a, b| b.share.total_cmp(&a.share));
    let thrown: Vec<String> = all
        .iter()
        .filter(|pop| {
            pop.area >= BIG && pop.share > ARRIVED && !answers_at_once(pop.scene, &pop.act)
        })
        .map(|pop| {
            format!(
                "{:.2} of {:.1}%: {} {}",
                pop.share,
                pop.area * 100.0,
                pop.scene,
                pop.act
            )
        })
        .collect();
    assert!(thrown.is_empty(), "these appear at once: {thrown:#?}");
}

#[test]
fn a_key_card_unfolds_over_the_list_and_folds_away_taking_nothing_as_it_goes() {
    let mut app = NativeApp::new(Some(fixture_vault("key-fold", true)));
    keychain(&mut app);
    let mut state = UiState::default();
    let output = step(&mut app, &mut state, &[], 0.0);
    let row = "Work laptop";
    let rest = place_of(&output, row).y;
    let click = click_on(&output, "Generate key");
    drive(&mut app, &mut state, &click, 1.0);
    let at = |output: &FrameOutput| place_of(output, row).y;
    let early = at(&drive(&mut app, &mut state, &[], 1.034));
    let later = at(&drive(&mut app, &mut state, &[], 1.1));
    let open = at(&drive(&mut app, &mut state, &[], 2.0));
    assert!(
        rest <= early && early < later && later < open,
        "the list glides down: {rest} {early} {later} {open}"
    );
    assert!(open - rest > 100.0, "it makes room for the card");
    let escape = [Event::Key(Key::Escape, Modifiers::NONE)];
    drive(&mut app, &mut state, &escape, 3.0);
    let going = drive(&mut app, &mut state, &[], 3.05);
    assert!(
        at(&going) > rest && at(&going) <= open,
        "the list glides back"
    );
    assert!(
        going
            .zones
            .iter()
            .all(|zone| zone.label != "Name, e.g. work laptop"),
        "what is on its way out takes nothing"
    );
    drive(&mut app, &mut state, &[], 4.0);
    let closed = drive(&mut app, &mut state, &[], 4.5);
    assert!((at(&closed) - rest).abs() < 0.5, "it ends where it started");
    assert!(!closed.repaint, "and nothing keeps asking for frames");
}

#[test]
fn under_reduced_motion_a_key_card_is_simply_there() {
    let mut app = NativeApp::new(Some(fixture_vault("key-fold-reduced", true)));
    keychain(&mut app);
    app.settings.reduce_motion = true;
    let mut state = UiState::default();
    let output = step(&mut app, &mut state, &[], 0.0);
    drive(
        &mut app,
        &mut state,
        &click_on(&output, "Generate key"),
        1.0,
    );
    let open = drive(&mut app, &mut state, &[], 1.001);
    assert!(
        open.zones
            .iter()
            .any(|zone| zone.label == "Name, e.g. work laptop"),
        "the frame that opens it shows it"
    );
}

#[test]
fn a_new_key_rises_in_and_a_deleted_one_goes_while_the_others_close_up() {
    let mut app = NativeApp::new(Some(fixture_vault("key-rows", true)));
    keychain(&mut app);
    let mut state = UiState::default();
    let output = step(&mut app, &mut state, &[], 0.0);
    let first = place_of(&output, "Work laptop").y;
    let vault = app.vault.as_mut().unwrap();
    let pair = annulus_ssh::keys::generate(annulus_ssh::keys::KeyKind::Ed25519, "me@test").unwrap();
    let key = crate::keychain_screen::stored(vault, "Zed key", &pair);
    vault.upsert_key(key).unwrap();
    let rising = place_of(&drive(&mut app, &mut state, &[], 1.0), "Zed key").y;
    assert!(lists_gliding(&app, 1.1), "the layout checks wait for it");
    drive(&mut app, &mut state, &[], 2.0);
    let landed = place_of(&drive(&mut app, &mut state, &[], 2.5), "Zed key").y;
    assert!(rising > landed, "it comes up into place: {rising} {landed}");
    assert!(
        !lists_gliding(&app, 2.6),
        "and look again once it has landed"
    );
    // The row above it goes: this one glides up into its place.
    let vault = app.vault.as_mut().unwrap();
    let gone = vault
        .ssh_keys()
        .iter()
        .find(|key| key.label == "Work laptop")
        .unwrap()
        .id;
    vault.remove_key(gone).unwrap();
    let moving = drive(&mut app, &mut state, &[], 3.0);
    let closing = place_of(&drive(&mut app, &mut state, &[], 3.1), "Zed key").y;
    assert!(moving.repaint, "the row that went is still fading");
    assert!(
        first < closing && closing < landed,
        "it glides: {first} {closing} {landed}"
    );
    drive(&mut app, &mut state, &[], 4.0);
    let there = drive(&mut app, &mut state, &[], 4.5);
    assert!((place_of(&there, "Zed key").y - first).abs() < 0.5);
    assert!(!there.repaint, "and nothing keeps asking for frames");
}

/// What film strips are photographed with: what the shipped app draws with on this OS.
fn film_renderer() -> Box<dyn Renderer> {
    #[cfg(windows)]
    {
        Box::new(Direct2D(annulus_platform::d2d::D2d::new().unwrap()))
    }
    #[cfg(not(windows))]
    {
        Box::new(Cpu(None))
    }
}

/// Draws frames of `app` for `film`, one for each of `times`, keeping each as a photograph;
/// returns what each frame output.
fn photographed(
    app: &mut NativeApp,
    film: &str,
    times: &[f64],
    events: &[Event],
) -> Vec<FrameOutput> {
    let dir = out_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let mut renderer = film_renderer();
    let mut state = UiState::default();
    let viewport = Rect::new(0.0, 0.0, 1100.0, 720.0);
    let theme = Theme::new(Appearance::Dark);
    let mut outputs = Vec::new();
    for (index, time) in times.iter().enumerate() {
        let mut output = FrameOutput::default();
        let first = if index == 0 { events } else { &[] };
        let png = renderer.frame((1100, 720, 1.0), true, &mut |canvas| {
            state.begin_frame(first, *time);
            let mut ui = Ui::new(canvas, theme, &mut state, viewport);
            app.frame(&mut ui);
            output = state.end_frame();
        });
        std::fs::write(dir.join(format!("{film}-{index}.png")), png.unwrap()).unwrap();
        outputs.push(output);
    }
    outputs
}

#[test]
fn the_vault_opens_as_the_lock_leaves_and_the_leaving_lock_takes_nothing() {
    let vault = fixture_vault("motion-unlock", true);
    let mut app = NativeApp::new(None);
    // A few frames locked, so the navigation rests dim first.
    let _ = photographed(&mut app, "motion-unlock-before", &[0.0, 0.5, 1.0], &[]);
    app.vault = Some(vault);
    app.lock_left = Some(2.0);
    let outputs = photographed(
        &mut app,
        "motion-unlock",
        &[2.04, 2.1, 2.18, 2.26, 2.34, 2.5, 3.0],
        &[],
    );
    // The vault works from the first frame, and nothing of the lock can be reached.
    for output in &outputs {
        assert!(output.zones.iter().any(|zone| zone.label == "New host"));
        assert!(!output.zones.iter().any(|zone| zone.label == "Unlock"));
    }
    assert!(outputs[0].repaint, "it asks for frames while it leaves");
    assert!(
        outputs[5].repaint,
        "the reveal is still moving half a second after success"
    );
    assert!(app.lock_left.is_none(), "and it is over");
}

#[test]
fn password_work_has_a_live_inert_unlock_state() {
    let mut app = NativeApp::new(None);
    app.lock.start_working(0.0);
    let outputs = photographed(
        &mut app,
        "motion-unlock-working",
        &[0.05, 0.35, 0.75, 1.15],
        &[],
    );
    for output in &outputs {
        assert!(output.repaint, "the processing motion keeps drawing");
        assert!(
            output
                .zones
                .iter()
                .any(|zone| zone.label == "Unlocking\u{2026}"),
            "the current state is named"
        );
        assert!(
            !output
                .zones
                .iter()
                .any(|zone| zone.label == "Master password"),
            "the password cannot change during the check"
        );
    }
}

#[test]
fn password_work_renders_in_each_theme_and_renderer() {
    let dir = out_dir();
    std::fs::create_dir_all(&dir).unwrap();
    for appearance in [Appearance::Dark, Appearance::Light] {
        for mut renderer in renderers() {
            let suffix = renderer.suffix();
            let look = match appearance {
                Appearance::Dark => "dark",
                Appearance::Light => "light",
            };
            let mut app = NativeApp::new(None);
            app.lock.start_working(0.0);
            let mut state = UiState::default();
            let viewport = Rect::new(0.0, 0.0, 1100.0, 720.0);
            let _ = renderer.frame((1100, 720, 1.0), false, &mut |canvas| {
                state.begin_frame(&[], 0.0);
                let mut ui = Ui::new(canvas, Theme::new(appearance), &mut state, viewport);
                app.frame(&mut ui);
                let _ = state.end_frame();
            });
            let mut output = FrameOutput::default();
            let png = renderer
                .frame((1100, 720, 1.0), true, &mut |canvas| {
                    state.begin_frame(&[], 0.35);
                    let mut ui = Ui::new(canvas, Theme::new(appearance), &mut state, viewport);
                    app.frame(&mut ui);
                    output = state.end_frame();
                })
                .unwrap();
            assert!(output.repaint);
            assert!(
                output
                    .zones
                    .iter()
                    .any(|zone| zone.label == "Unlocking\u{2026}")
            );
            std::fs::write(dir.join(format!("lock-unlocking-{look}{suffix}.png")), png).unwrap();
        }
    }
}

#[test]
fn reduced_motion_keeps_password_work_still() {
    let mut app = NativeApp::new(None);
    app.settings.reduce_motion = true;
    app.lock.start_working(0.0);
    let mut state = UiState::default();
    let _ = step(&mut app, &mut state, &[], 0.0);
    let settled = step(&mut app, &mut state, &[], 0.35);
    assert!(
        !settled.repaint,
        "the still state needs no animation frames"
    );
}

#[test]
fn a_refused_password_shakes_the_field_and_says_why() {
    let mut app = NativeApp::new(None);
    let _ = photographed(&mut app, "motion-refused-before", &[0.0, 0.5], &[]);
    app.lock.refuse("Wrong password.".into());
    let outputs = photographed(
        &mut app,
        "motion-refused",
        &[1.0, 1.06, 1.12, 1.2, 2.0],
        &[],
    );
    assert!(outputs[1].repaint, "it moves");
    let field = |output: &FrameOutput| {
        output
            .zones
            .iter()
            .find(|zone| zone.label == "Master password")
            .map(|zone| zone.rect.x)
            .unwrap()
    };
    let places: Vec<f32> = outputs.iter().map(field).collect();
    let tops: Vec<f32> = outputs
        .iter()
        .map(|output| {
            output
                .zones
                .iter()
                .find(|zone| zone.label == "Master password")
                .map(|zone| zone.rect.y)
                .unwrap()
        })
        .collect();
    assert!(
        tops.windows(2).all(|two| (two[0] - two[1]).abs() < 0.01),
        "nothing moves up or down for it: {tops:?}"
    );
    assert!(
        places[1..4].iter().any(|x| (x - places[4]).abs() > 2.0),
        "the field left its place: {places:?}"
    );
    assert!(!outputs[4].repaint, "and it comes to rest");
}

#[test]
fn a_host_flies_from_its_card_to_its_tab_and_lands() {
    let vault = fixture_vault("motion-flight", true);
    let mut app = NativeApp::new(Some(vault));
    with_tabs(&mut app);
    let uid = app.tabs.last().unwrap().uid;
    let mut renderer = film_renderer();
    let mut state = UiState::default();
    let viewport = Rect::new(0.0, 0.0, 1100.0, 720.0);
    let theme = Theme::new(Appearance::Dark);
    let dir = out_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let mut shoot = |app: &mut NativeApp, time: f64, name: Option<String>| {
        let mut output = FrameOutput::default();
        let png = renderer.frame((1100, 720, 1.0), name.is_some(), &mut |canvas| {
            state.begin_frame(&[], time);
            let mut ui = Ui::new(canvas, theme, &mut state, viewport);
            app.frame(&mut ui);
            output = state.end_frame();
        });
        if let (Some(name), Some(png)) = (name, png) {
            std::fs::write(dir.join(name), png).unwrap();
        }
        output
    };
    // The page at rest, as it is when a card is double-clicked.
    shoot(&mut app, 0.0, None);
    shoot(&mut app, 2.0, None);
    assert!(
        (app.tab_landing(uid, 2.0) - 1.0).abs() < f32::EPSILON,
        "nothing flying"
    );
    app.flyer = Some(Flyer {
        uid,
        from: Rect::new(244.0, 312.0, 266.0, 64.0),
        title: "bastion".into(),
        os: None,
        since: 3.0,
    });
    for (index, at) in [3.04, 3.12, 3.24, 3.4].into_iter().enumerate() {
        let output = shoot(&mut app, at, Some(format!("motion-flight-{index}.png")));
        assert!(output.repaint, "it asks for frames while it flies");
    }
    assert!(app.flyer.is_some(), "still in the air");
    assert!(app.tab_landing(uid, 3.1) < 0.01, "the tab's own name waits");
    assert!(app.tab_landing(uid, 3.4) > 0.5, "and comes up as it lands");
    shoot(&mut app, 3.5, None);
    assert!(app.flyer.is_none(), "landed");
}

/// One frame of `app` in a window `width` wide, drawing nothing.
fn frame_wide(state: &mut UiState, app: &mut NativeApp, width: f32, time: f64) -> FrameOutput {
    let mut canvas = Measure(TextEngine::new());
    state.begin_frame(&[], time);
    let mut ui = Ui::new(
        &mut canvas,
        Theme::new(Appearance::Dark),
        state,
        Rect::new(0.0, 0.0, width, 720.0),
    );
    app.frame(&mut ui);
    state.end_frame()
}

/// Where the zone named `label` is.
fn place_of(output: &FrameOutput, label: &str) -> Rect {
    output
        .zones
        .iter()
        .find(|zone| zone.label == label)
        .map(|zone| zone.rect)
        .unwrap()
}

#[test]
fn in_the_same_columns_the_grid_glides_when_the_editor_opens() {
    let vault = fixture_vault("motion-reflow", true);
    let mut app = NativeApp::new(Some(vault));
    let mut state = UiState::default();
    // Wide enough for four columns with the editor open or not.
    let width = 1800.0;
    let _ = frame_wide(&mut state, &mut app, width, 0.0);
    let before = place_of(&frame_wide(&mut state, &mut app, width, 1.0), "web-1");
    let id = app.vault.as_ref().unwrap().hosts()[0].id;
    app.hosts.edit_host(app.vault.as_ref().unwrap(), id);
    let first = place_of(&frame_wide(&mut state, &mut app, width, 1.016), "web-1");
    let later = place_of(&frame_wide(&mut state, &mut app, width, 1.1), "web-1");
    let settled = place_of(&frame_wide(&mut state, &mut app, width, 3.0), "web-1");
    assert_ne!(before, settled, "the editor took room");
    assert!(
        (first.x - before.x).abs() < (settled.x - before.x).abs() / 2.0,
        "it starts near where it was: {before:?} {first:?} {settled:?}"
    );
    assert!(
        (later.x - settled.x).abs() < (first.x - settled.x).abs(),
        "on its way: {first:?} {later:?} {settled:?}"
    );
}

#[test]
fn into_other_columns_the_grid_fades_through_instead_of_crossing_paths() {
    let vault = fixture_vault("motion-refill", true);
    let mut app = NativeApp::new(Some(vault));
    let mut state = UiState::default();
    let width = 1100.0;
    let _ = frame_wide(&mut state, &mut app, width, 0.0);
    let _ = frame_wide(&mut state, &mut app, width, 1.0);
    let id = app.vault.as_ref().unwrap().hosts()[0].id;
    app.hosts.edit_host(app.vault.as_ref().unwrap(), id);
    // No card travels: each is in its new place from the first frame.
    let first = place_of(&frame_wide(&mut state, &mut app, width, 1.016), "web-1");
    let settled = place_of(&frame_wide(&mut state, &mut app, width, 3.0), "web-1");
    assert!(
        (first.center().x - settled.center().x).abs() < 0.5,
        "{first:?} {settled:?}"
    );
    assert!(!app.hosts.reflowing(1.016));
}

#[test]
fn a_window_being_resized_moves_the_grid_at_once() {
    let vault = fixture_vault("motion-resize", true);
    let mut app = NativeApp::new(Some(vault));
    let mut state = UiState::default();
    let _ = frame_wide(&mut state, &mut app, 1100.0, 0.0);
    let _ = frame_wide(&mut state, &mut app, 1100.0, 1.0);
    let wide = place_of(&frame_wide(&mut state, &mut app, 1500.0, 1.016), "web-1");
    let settled = place_of(&frame_wide(&mut state, &mut app, 1500.0, 3.0), "web-1");
    assert_eq!(wide, settled);
}

#[test]
fn transitions_move_through_frames_and_are_photographed_mid_flight() {
    let dir = out_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let mut renderer = film_renderer();
    let mut still = Vec::new();
    for film in FILMS {
        let frames = filmstrip(film, &mut *renderer);
        for (index, png) in frames.iter().enumerate() {
            std::fs::write(dir.join(format!("{}-{index}.png", film.name)), png).unwrap();
        }
        // Moving: the first frames differ from each other, and the last is where it settles.
        if frames[0] == frames[1] || frames[1] == frames[2] {
            still.push(film.name);
        }
    }
    assert!(still.is_empty(), "these did not move: {still:?}");
}

/// The renderers every scene is drawn with: ours, and Direct2D on Windows. Each thread makes
/// its own (Direct2D's factory is single-threaded).
fn renderers() -> Vec<Box<dyn Renderer>> {
    vec![
        Box::new(Cpu(None)),
        #[cfg(windows)]
        Box::new(Direct2D(annulus_platform::d2d::D2d::new().unwrap())),
    ]
}

/// How many threads a heavy test spreads its independent work over: every core, but no more
/// than 8, since the CI runner runs other jobs beside this one.
fn workers() -> usize {
    std::thread::available_parallelism().map_or(4, |n| n.get().min(8))
}

/// The agents' tests time a real shell's prompt (a pause of `control::QUIET` with no prompt
/// reads as a command waiting for input). Rendering every scene or running the random users
/// on all cores beside them starved PowerShell on the Windows runner, so those run alone;
/// the agents' tests share this with each other.
static TIMED_SHELLS: std::sync::RwLock<()> = std::sync::RwLock::new(());

/// Holds off the scene renders while an agent's shell is being timed.
fn timing_a_shell() -> std::sync::RwLockReadGuard<'static, ()> {
    TIMED_SHELLS
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[test]
fn every_scene_renders_without_layout_faults() {
    let _alone = TIMED_SHELLS
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = out_dir();
    std::fs::create_dir_all(&dir).unwrap();
    // `ANNULUS_SCENES=forwards` renders only the scenes whose names contain it.
    let only = std::env::var("ANNULUS_SCENES").ok();
    let jobs: Vec<(&Scene, Appearance, usize)> = (0..renderers().len())
        .flat_map(|renderer| {
            SCENES
                .iter()
                .filter(|scene| only.as_deref().is_none_or(|part| scene.name.contains(part)))
                .flat_map(move |scene| {
                    [Appearance::Dark, Appearance::Light].map(|look| (scene, look, renderer))
                })
        })
        .collect();
    // Each render has its own vault (named by scene, look and renderer), so they run side by
    // side: thread n takes every n-th one, which spreads the heavy scenes out.
    let threads = workers();
    let failures: Vec<String> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|first| {
                let (jobs, dir) = (&jobs, &dir);
                scope.spawn(move || {
                    let mut renderers = renderers();
                    let mut failures = Vec::new();
                    for &(scene, appearance, index) in jobs.iter().skip(first).step_by(threads) {
                        let renderer = renderers[index].as_mut();
                        let (png, faults) = render(scene, appearance, renderer);
                        let theme = match appearance {
                            Appearance::Dark => "dark",
                            Appearance::Light => "light",
                        };
                        let name = format!("{}-{theme}{}", scene.name, renderer.suffix());
                        std::fs::write(dir.join(format!("{name}.png")), png).unwrap();
                        failures.extend(faults.into_iter().map(|fault| format!("{name}: {fault}")));
                    }
                    failures
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().unwrap())
            .collect()
    });
    assert!(
        failures.is_empty(),
        "layout faults:
{}",
        failures.join(
            "
"
        )
    );
}

/// Draws one frame of `app` with input `events`; returns what the toolkit reported.
fn drive(app: &mut NativeApp, state: &mut UiState, events: &[Event], time: f64) -> FrameOutput {
    let mut renderer = Cpu(None);
    let viewport = Rect::new(0.0, 0.0, 1100.0, 800.0);
    let mut output = FrameOutput::default();
    renderer.frame((1100, 800, 1.0), false, &mut |canvas| {
        state.begin_frame(events, time);
        let mut ui = Ui::new(canvas, Theme::new(Appearance::Dark), state, viewport);
        app.frame(&mut ui);
        output = state.end_frame();
    });
    output
}

/// Draws a frame with `events`, then the frame after input that the window draws too.
fn step(app: &mut NativeApp, state: &mut UiState, events: &[Event], time: f64) -> FrameOutput {
    drive(app, state, events, time);
    drive(app, state, &[], time);
    drive(app, state, &[], time + 0.4)
}

/// Lays out with the real fonts but draws no pixels: the random-input test runs thousands of
/// frames, and only what the toolkit decides matters there.
struct Measure(TextEngine);

impl Canvas for Measure {
    fn scale(&self) -> f32 {
        1.0
    }
    fn fill_rect(&mut self, _: Rect, _: f32, _: annulus_ui::gfx::Color) {}
    fn stroke_rect(&mut self, _: Rect, _: f32, _: f32, _: annulus_ui::gfx::Color) {}
    fn shadow(&mut self, _: Rect, _: f32, _: f32, _: f32, _: annulus_ui::gfx::Color) {}
    fn fill_circle(&mut self, _: Point, _: f32, _: annulus_ui::gfx::Color) {}
    fn polyline(&mut self, _: &[Point], _: bool, _: f32, _: annulus_ui::gfx::Color) {}
    fn stroke_circle(&mut self, _: Point, _: f32, _: f32, _: annulus_ui::gfx::Color) {}
    fn fill_polygon(&mut self, _: &[Point], _: annulus_ui::gfx::Color) {}
    fn measure(&mut self, text: &str, font: annulus_ui::gfx::Font) -> annulus_ui::gfx::TextSize {
        self.0.measure(text, font)
    }
    fn draw_text(
        &mut self,
        _: Point,
        _: &str,
        _: annulus_ui::gfx::Font,
        _: annulus_ui::gfx::Color,
    ) {
    }
    fn push_clip(&mut self, _: Rect) {}
    fn pop_clip(&mut self) {}
}

/// A seeded generator (xorshift64*), so a failing run replays from its seed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        #[allow(clippy::cast_possible_truncation)] // A list index.
        let at = (self.next() % n.max(1) as u64) as usize;
        at
    }
}

/// What the random user types: plain words, addresses, accents, other scripts, emoji, a
/// combining mark, spaces, a very long name and several lines.
const TYPED: &[&str] = &[
    "a",
    "web-1",
    "root@10.0.4.12:2222",
    "\u{c4}rger.txt",
    "\u{65e5}\u{672c}\u{8a9e}",
    "\u{1f642}",
    "e\u{301}",
    " ",
    "--",
    "a-very-long-host-name-that-goes-on-and-on-and-on.internal.example.com",
    "644",
    "line one\nline two",
];

/// Controls the random user leaves alone: they reach the network or other machines, write
/// outside the test's folders, or lock the vault for the rest of the run.
const HANDS_OFF: &[&str] = &[
    "Create account",
    "Sign in",
    "Sync now",
    "Export to Downloads",
    "Lock",
    "Install",
    "Run this rule",
    "Download to this computer",
    "Upload to the host",
];

/// One random action against the last frame's zones; also says what it was, for the report.
fn random_action(rng: &mut Rng, zones: &[Zone]) -> (Vec<Event>, String) {
    let targets: Vec<&Zone> = zones
        .iter()
        .filter(|zone| zone.role != Role::WindowButton && zone.rect.w > 1.0)
        .filter(|zone| !HANDS_OFF.iter().any(|name| zone.label.starts_with(name)))
        .collect();
    let click = |zone: &Zone, twice: bool| {
        let at = zone.rect.center();
        let mut events = vec![Event::PointerMoved(at)];
        for _ in 0..if twice { 2 } else { 1 } {
            events.push(Event::PointerDown(Button::Primary));
            events.push(Event::PointerUp(Button::Primary));
        }
        events
    };
    match rng.below(24) {
        0..=9 if !targets.is_empty() => {
            let zone = targets[rng.below(targets.len())];
            (click(zone, false), format!("click {:?}", zone.label))
        }
        10 if !targets.is_empty() => {
            let zone = targets[rng.below(targets.len())];
            (click(zone, true), format!("double-click {:?}", zone.label))
        }
        11..=13 => {
            let text = TYPED[rng.below(TYPED.len())];
            (vec![Event::Text(text.to_owned())], format!("type {text:?}"))
        }
        14 => {
            let text = TYPED[rng.below(TYPED.len())];
            (
                vec![Event::Paste(text.to_owned())],
                format!("paste {text:?}"),
            )
        }
        15 => (vec![Event::Key(Key::Tab, Modifiers::NONE)], "Tab".into()),
        16 => (
            vec![Event::Key(Key::Tab, Modifiers::SHIFT)],
            "Shift+Tab".into(),
        ),
        17 => (
            vec![Event::Key(Key::Escape, Modifiers::NONE)],
            "Escape".into(),
        ),
        18 => (
            vec![Event::Key(Key::Enter, Modifiers::NONE)],
            "Enter".into(),
        ),
        19 => {
            let key = [Key::Backspace, Key::Delete, Key::F(2)][rng.below(3)];
            (vec![Event::Key(key, Modifiers::NONE)], format!("{key:?}"))
        }
        20 => {
            let key = [
                Key::Left,
                Key::Right,
                Key::Up,
                Key::Down,
                Key::Home,
                Key::End,
            ][rng.below(6)];
            let modifiers = [Modifiers::NONE, Modifiers::SHIFT, Modifiers::CTRL][rng.below(3)];
            (
                vec![Event::Key(key, modifiers)],
                format!("{key:?} {modifiers:?}"),
            )
        }
        21 if !targets.is_empty() => {
            let zone = targets[rng.below(targets.len())];
            (
                vec![
                    Event::PointerMoved(zone.rect.center()),
                    Event::Wheel { x: 0.0, y: -120.0 },
                ],
                format!("scroll over {:?}", zone.label),
            )
        }
        _ => {
            let shortcut = [
                shortcuts::PALETTE,
                shortcuts::NEW_TAB,
                shortcuts::CLOSE,
                shortcuts::FIND,
                shortcuts::FOCUS_MODE,
                shortcuts::BROADCAST,
                shortcuts::SPLIT_RIGHT,
                shortcuts::SPLIT_DOWN,
                shortcuts::SNIPPETS,
            ][rng.below(9)];
            (vec![chord(shortcut)], shortcut.keys())
        }
    }
}

/// One frame of `app` on `canvas` in a window of `viewport`.
fn measured_frame(
    canvas: &mut Measure,
    state: &mut UiState,
    app: &mut NativeApp,
    (events, time): (&[Event], f64),
    viewport: Rect,
) -> FrameOutput {
    state.begin_frame(events, time);
    let mut ui = Ui::new(canvas, Theme::new(Appearance::Dark), state, viewport);
    app.frame(&mut ui);
    state.end_frame()
}

/// Drives the whole app with `steps` random actions from `seed`: every frame must be
/// free of panics, layout faults, accessibility-tree faults and shared zone ids. New terminal
/// and SFTP tabs are closed before they first draw, so nothing connects anywhere.
fn random_user(seed: u64, steps: usize, (width, height): (f32, f32)) {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    let mut vault = fixture_vault(&format!("monkey-{seed}"), true);
    // Rules that start by themselves would connect to the made-up hosts.
    for rule in vault.forwards().to_vec() {
        let rule = annulus_vault::forwards::PortForward {
            auto_start: false,
            ..rule
        };
        vault.upsert_forward(rule).unwrap();
    }
    let mut app = NativeApp::new(Some(vault));
    app.ssh_files.known_hosts = crate::native::demo::known_hosts(&format!("monkey-{seed}"));
    app.ssh_files.key_dir = None;
    app.settings.auto_lock_minutes = 0;
    app.push_sftp(crate::native::demo::sftp_tab(&format!("monkey-{seed}")));
    app.push_terminal(
        "replay",
        TermView::Replay(Box::new(crate::native::demo::terminal())),
    );
    app.active = 0;
    let keep: Vec<u64> = app.tabs.iter().map(|tab| tab.uid).collect();
    let viewport = Rect::new(0.0, 0.0, width, height);
    let mut canvas = Measure(TextEngine::new());
    let mut state = UiState::default();
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mut zones: Vec<Zone> = Vec::new();
    let mut history: Vec<String> = Vec::new();
    // What the run reached, printed at the end so a narrow run shows.
    let mut reached: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut sections = std::collections::BTreeSet::new();
    for step in 0..steps {
        let (events, what) = random_action(&mut rng, &zones);
        history.push(what);
        if history.len() > 16 {
            history.remove(0);
        }
        #[allow(clippy::cast_precision_loss)] // Step counts are small.
        let time = step as f64 * 0.25;
        let frames = catch_unwind(AssertUnwindSafe(|| {
            let first =
                measured_frame(&mut canvas, &mut state, &mut app, (&events, time), viewport);
            // Nothing new connects: terminal and SFTP tabs opened by the step go before they draw.
            app.tabs
                .retain(|tab| keep.contains(&tab.uid) || tab.kind == crate::native::TabKind::Vault);
            app.active = app.active.min(app.tabs.len().saturating_sub(1));
            let second = measured_frame(
                &mut canvas,
                &mut state,
                &mut app,
                (&[], time + 0.1),
                viewport,
            );
            (first, second)
        }));
        let Ok((first, second)) = frames else {
            panic!("seed {seed}, step {step}: a panic after {history:#?}");
        };
        let mut faults = first.faults;
        faults.extend(second.faults);
        // Cards and rows gliding to new places pass over each other for a moment.
        if !app.hosts.reflowing(time + 0.1) && !lists_gliding(&app, time + 0.1) {
            faults.extend(layout_faults(&second.zones, viewport));
        }
        let tree = annulus_ui::access::tree(&second.zones, state.focus(), 1.0, "Annulus");
        faults.extend(annulus_ui::access::faults(&tree));
        let mut ids = std::collections::HashSet::new();
        for zone in &second.zones {
            if !ids.insert(zone.id) {
                faults.push(format!("{:?} shares its id", zone.label));
            }
        }
        assert!(
            faults.is_empty(),
            "seed {seed}, step {step}: {faults:#?}\nafter: {history:#?}"
        );
        reached.extend(second.zones.iter().map(|zone| zone.label.clone()));
        sections.insert(format!("{:?}", app.section));
        zones = second.zones;
    }
    eprintln!(
        "seed {seed}: {steps} steps, {} distinct controls, sections {sections:?}",
        reached.len()
    );
}

/// Whether the rows of any of `app`'s lists are gliding to new places at `now`.
fn lists_gliding(app: &NativeApp, now: f64) -> bool {
    crate::native::screens::lists_gliding(
        (&app.keychain, &app.snippets, &app.forwards),
        (&app.known, &app.logs),
        now,
    )
}

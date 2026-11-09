/// End to end through the deployed sync server: the new UI's Settings create an account
/// from typed input (server, name, the vault's password) and the vault syncs to it.
#[test]
#[ignore = "needs the deployed sync server (ANNULUS_E2E_SYNC_URL)"]
fn e2e_the_new_settings_create_a_sync_account_and_sync() {
    let url = std::env::var("ANNULUS_E2E_SYNC_URL").expect("ANNULUS_E2E_SYNC_URL");
    // An account on a server needs a master password that is hard to guess.
    let strong = "violet-anchor-pebble-tundra";
    let mut vault = fixture_vault("e2e-account", true);
    vault.change_password(strong).unwrap();
    let path = vault.path().to_path_buf();
    let mut app = NativeApp::new(Some(vault));
    app.vault_path = Some(path);
    app.section = crate::native::Section::Settings;
    let mut state = UiState::default();
    let started = std::time::Instant::now();
    let clock = || started.elapsed().as_secs_f64();
    let mut output = drive(&mut app, &mut state, &[], clock());
    let name = format!("e2e-{}", std::process::id());
    let clear_device = vec![
        Event::Key(Key::Char('a'), Modifiers::COMMAND),
        Event::Key(Key::Backspace, Modifiers::NONE),
    ];
    for (field, text) in [
        ("Server", url.as_str()),
        ("Account name", name.as_str()),
        ("This device's name", "e2e device"),
        ("This vault's master password", strong),
    ] {
        let mut events = vec![
            Event::PointerMoved(zone_center(&output.zones, field)),
            Event::PointerDown(Button::Primary),
            Event::PointerUp(Button::Primary),
        ];
        drive(&mut app, &mut state, &events, clock());
        events = clear_device.clone();
        events.push(Event::Text(text.to_owned()));
        output = drive(&mut app, &mut state, &events, clock());
    }
    let press_on = |zones: &[Zone], name: &str| {
        vec![
            Event::PointerMoved(zone_center(zones, name)),
            Event::PointerDown(Button::Primary),
            Event::PointerUp(Button::Primary),
        ]
    };
    let events = press_on(&output.zones, "Create account");
    drive(&mut app, &mut state, &events, clock());
    loop {
        assert!(clock() < 90.0, "no sync; message: {:?}", app.sync.message);
        drive(&mut app, &mut state, &[], clock());
        let synced = app.vault.as_ref().is_some_and(|vault| {
            let state = vault.sync_state();
            state.account.is_some() && state.last_synced > 0 && state.pending_count() == 0
        });
        if synced {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert_eq!(app.sync.message.as_ref().filter(|(error, _)| *error), None);
}

#[test]
fn tab_crosses_the_hosts_toolbar_left_to_right() {
    let mut app = NativeApp::new(Some(fixture_vault("tab-order", true)));
    let mut state = UiState::default();
    let mut renderer = Cpu(None);
    let viewport = Rect::new(0.0, 0.0, 1100.0, 720.0);
    let mut focused = Vec::new();
    for step in 0..40 {
        let events = if step == 0 {
            Vec::new()
        } else {
            vec![Event::Key(Key::Tab, Modifiers::NONE)]
        };
        let mut output = FrameOutput::default();
        renderer.frame((1100, 720, 1.0), false, &mut |canvas| {
            #[allow(clippy::cast_precision_loss)] // Step counts are small.
            state.begin_frame(&events, f64::from(step));
            let mut ui = Ui::new(canvas, Theme::new(Appearance::Dark), &mut state, viewport);
            app.frame(&mut ui);
            output = state.end_frame();
        });
        if let Some(id) = state.focus()
            && let Some(zone) = output.zones.iter().find(|zone| zone.id == id)
        {
            focused.push(zone.label.clone());
        }
    }
    let at = |label: &str| {
        focused
            .iter()
            .position(|seen| seen == label)
            .unwrap_or_else(|| panic!("{label:?} never focused: {focused:?}"))
    };
    let order = [
        at("Vault"),
        at("Hosts"),
        at("Settings"),
        at("Grid view"),
        at("List view"),
        at("Tags"),
        at("Terminal"),
        at("New host"),
        at("More ways to add hosts"),
        at("Search hosts, or type user@host:port to connect"),
        at("Homelab"),
    ];
    assert!(order.is_sorted(), "{order:?}: {focused:?}");
}

#[test]
fn the_lint_catches_partial_overlaps_but_not_nesting() {
    let zone = |label: &str, rect: Rect| {
        Zone::new(annulus_ui::kit::Id::new(label), rect, Role::Button, label)
    };
    let viewport = Rect::new(0.0, 0.0, 100.0, 100.0);
    let card = zone("card", Rect::new(0.0, 0.0, 50.0, 50.0));
    let inner = zone("inner", Rect::new(10.0, 10.0, 10.0, 10.0));
    let across = zone("across", Rect::new(40.0, 0.0, 20.0, 20.0));
    let outside = zone("outside", Rect::new(90.0, 90.0, 20.0, 20.0));
    assert_eq!(
        layout_faults(&[card.clone(), inner], viewport),
        Vec::<String>::new()
    );
    assert_eq!(
        layout_faults(&[card.clone(), across.clone()], viewport).len(),
        1
    );
    assert_eq!(layout_faults(&[outside], viewport).len(), 1);
    // A menu over the card, above a backdrop, is fine.
    let backdrop = Zone {
        role: Role::Backdrop,
        ..zone("backdrop", viewport)
    };
    assert_eq!(
        layout_faults(&[card, backdrop, across], viewport),
        Vec::<String>::new()
    );
}

#[test]
fn the_tab_order_lint_catches_a_mirrored_row_but_not_a_new_column() {
    let zone = |label: &str, rect: Rect| {
        Zone::new(annulus_ui::kit::Id::new(label), rect, Role::Button, label)
    };
    let nav = zone("nav", Rect::new(0.0, 500.0, 100.0, 30.0));
    let content = zone("content", Rect::new(200.0, 50.0, 100.0, 30.0));
    assert_eq!(
        tab_order_faults(&[nav, content.clone()]),
        Vec::<String>::new()
    );
    let right = zone("right", Rect::new(400.0, 50.0, 80.0, 30.0));
    let left = zone("left", Rect::new(300.0, 50.0, 80.0, 30.0));
    assert_eq!(tab_order_faults(&[right.clone(), left.clone()]).len(), 1);
    let backdrop = Zone {
        role: Role::Backdrop,
        ..zone("backdrop", Rect::new(0.0, 0.0, 800.0, 600.0))
    };
    assert_eq!(
        tab_order_faults(&[right, backdrop, left]),
        Vec::<String>::new()
    );
}

#[test]
fn first_run_creates_the_vault_and_opens_it_only_after_the_recovery_key_is_confirmed() {
    let dir = std::env::temp_dir().join(format!("annulus-first-run-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = NativeApp::new(None);
    app.vault_path = Some(dir.join("vault.anv"));
    app.lock = crate::native::lock::LockView::new(crate::native::lock::Mode::Setup);
    let mut state = UiState::default();
    let started = std::time::Instant::now();
    let clock = || started.elapsed().as_secs_f64();
    let mut output = drive(&mut app, &mut state, &[], clock());
    let type_into = |output: &FrameOutput, field: &str, text: &str| {
        vec![
            Event::PointerMoved(zone_center(&output.zones, field)),
            Event::PointerDown(Button::Primary),
            Event::PointerUp(Button::Primary),
            Event::Text(text.to_owned()),
        ]
    };
    let events = type_into(
        &output,
        "Master password (10+ characters)",
        "correct horse battery",
    );
    output = drive(&mut app, &mut state, &events, clock());
    let events = type_into(&output, "Type it again", "correct horse battery");
    output = drive(&mut app, &mut state, &events, clock());
    let create = vec![
        Event::PointerMoved(zone_center(&output.zones, "Create vault")),
        Event::PointerDown(Button::Primary),
        Event::PointerUp(Button::Primary),
    ];
    output = drive(&mut app, &mut state, &create, clock());
    while app.lock.mode != crate::native::lock::Mode::Recovery {
        assert!(
            clock() < 60.0,
            "the vault was not created: {:?}",
            app.lock.error
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
        output = drive(&mut app, &mut state, &[], clock());
    }
    assert!(app.vault.is_none(), "not open before the key is confirmed");
    // Copying the key copies it as a secret: kept out of clipboard history, cleared later.
    output = drive(
        &mut app,
        &mut state,
        &click_on(&output, "Copy the recovery key"),
        clock(),
    );
    assert_eq!(
        state.take_copied(),
        Some((app.lock.recovery_key_for_tests(), true))
    );
    // A wrong first group is refused.
    let events = type_into(
        &output,
        "Type its first group to confirm you saved it",
        "nope",
    );
    output = drive(&mut app, &mut state, &events, clock());
    let confirm = |output: &FrameOutput| {
        vec![
            Event::PointerMoved(zone_center(&output.zones, "I saved it, open the vault")),
            Event::PointerDown(Button::Primary),
            Event::PointerUp(Button::Primary),
        ]
    };
    output = drive(&mut app, &mut state, &confirm(&output), clock());
    assert!(app.vault.is_none());
    // The key's first group, from the file the vault now has on disk: reopen proves it.
    assert!(dir.join("vault.anv").exists());
    let key = app.lock.recovery_key_for_tests();
    let first = key.split('-').next().unwrap().to_owned();
    let mut events = vec![
        Event::PointerMoved(zone_center(
            &output.zones,
            "Type its first group to confirm you saved it",
        )),
        Event::PointerDown(Button::Primary),
        Event::PointerUp(Button::Primary),
        Event::Key(Key::Char('a'), Modifiers::COMMAND),
        Event::Key(Key::Backspace, Modifiers::NONE),
    ];
    events.push(Event::Text(first));
    output = drive(&mut app, &mut state, &events, clock());
    drive(&mut app, &mut state, &confirm(&output), clock());
    assert!(app.vault.is_some());
}

#[test]
fn closing_a_tab_saves_its_log_only_when_logging_is_on() {
    for record in [false, true] {
        let mut app = NativeApp::new(Some(fixture_vault(&format!("logs-{record}"), false)));
        app.settings.record_logs = record;
        let program = if cfg!(windows) {
            annulus_session::Program::Command {
                program: "cmd.exe".into(),
                args: vec!["/q".into(), "/k".into(), "echo annulus-log-line".into()],
            }
        } else {
            annulus_session::Program::Command {
                program: "sh".into(),
                args: vec!["-c".into(), "echo annulus-log-line; exec sh".into()],
            }
        };
        app.open("Shell", program);
        let mut state = UiState::default();
        let started = std::time::Instant::now();
        let mut seen = false;
        while !seen {
            assert!(
                started.elapsed().as_secs() < 20,
                "the shell printed nothing"
            );
            drive(&mut app, &mut state, &[], started.elapsed().as_secs_f64());
            seen = app
                .tabs
                .last()
                .and_then(|tab| match &tab.terminal {
                    Some(crate::native::TermView::Live(panes)) => Some(panes.transcripts()),
                    _ => None,
                })
                .is_some_and(|logs| {
                    logs.iter()
                        .any(|(_, text)| text.contains("annulus-log-line"))
                });
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        app.close_tab(app.tabs.len() - 1);
        let logs = app.vault.as_ref().unwrap().logs();
        assert_eq!(
            logs.iter().any(|log| log.text.contains("annulus-log-line")),
            record,
            "record_logs = {record}"
        );
    }
}

#[test]
fn a_log_of_a_tab_closed_while_locked_is_saved_once_the_vault_opens() {
    let mut app = NativeApp::new(Some(fixture_vault("logs-locked", false)));
    app.settings.record_logs = true;
    replay(&mut app, "build");
    let vault = app.vault.take();
    app.save_logs(vec![("build".into(), "make: done".into())]);
    assert!(app.vault.is_none());
    app.vault = vault;
    app.unlocked();
    let logs = app.vault.as_ref().unwrap().logs();
    assert!(logs.iter().any(|log| log.text == "make: done"), "{logs:?}");
}

#[test]
fn tabs_are_listed_for_the_next_launch_and_host_tabs_wait_for_the_vault() {
    let mut app = NativeApp::new(Some(fixture_vault("restore", true)));
    app.settings.restore_tabs = true;
    let host = app.vault.as_ref().unwrap().hosts()[0].id;
    app.act(crate::native::hosts::HostsAction::Connect(host));
    // A tab with no origin to reopen it from is not listed.
    app.push_terminal("plain", TermView::Replay(Box::new(crate::native::demo::terminal())));
    let mut state = UiState::default();
    drive(&mut app, &mut state, &[], 0.0);
    assert_eq!(
        app.settings.open_tabs,
        [annulus_config::settings::OpenTab::Host(host)]
    );
    // The next start: a host tab waits for the vault, then reconnects.
    let mut next = NativeApp::new(None);
    next.settings = app.settings.clone();
    next.restore();
    assert_eq!(next.pending_hosts, [host]);
    next.vault = app.vault.take();
    drive(&mut next, &mut state, &[], 1.0);
    drive(&mut next, &mut state, &[], 2.0);
    assert_eq!(next.pending_hosts, Vec::<u64>::new());
    assert_eq!(next.tabs.len(), 3, "the host's tab is open again");
    assert_eq!(next.active, 2);
    // Quitting at the lock screen keeps a host tab that is still waiting for the vault.
    let mut locked = NativeApp::new(None);
    locked.settings = next.settings.clone();
    locked.restore();
    drive(&mut locked, &mut state, &[], 3.0);
    assert_eq!(
        locked.settings.open_tabs,
        [annulus_config::settings::OpenTab::Host(host)]
    );
    // With the setting off, nothing comes back.
    let mut off = NativeApp::new(None);
    off.settings = next.settings.clone();
    off.settings.restore_tabs = false;
    off.restore();
    assert!(off.pending_hosts.is_empty() && off.opens.is_empty());
}

fn replay(app: &mut NativeApp, title: &str) {
    app.push_terminal(title, TermView::Replay(Box::new(crate::native::demo::terminal())));
}

fn titles(app: &NativeApp) -> Vec<&str> {
    app.tabs.iter().map(|tab| tab.title.as_str()).collect()
}

#[test]
fn the_palette_lists_tabs_and_commands_and_runs_them() {
    use crate::command_palette::Action;
    let mut app = NativeApp::new(None);
    replay(&mut app, "build");
    app.active = 0;
    let items = app.palette_items();
    let kinds: Vec<(&str, &str)> = items
        .iter()
        .map(|item| (item.label.as_str(), item.kind))
        .collect();
    assert_eq!(
        kinds[0],
        ("build", "Tab"),
        "locked: no hosts, the tabs come first"
    );
    assert!(kinds.contains(&("Settings", "Command")));
    assert!(kinds.contains(&("SFTP", "Page")));
    app.run_action(Action::Tab(2));
    assert_eq!(app.active, 2);
    app.run_action(Action::Settings(crate::native::settings::Page::Appearance));
    assert_eq!((app.active, app.section), (0, crate::native::Section::Settings));
    app.run_action(Action::NewLocal);
    assert_eq!(app.opens.len(), 1, "a local shell opens after this frame");
}

#[test]
fn the_palette_lists_every_page_and_opens_host_and_key_editors() {
    use crate::command_palette::Action;
    let mut app = NativeApp::new(Some(fixture_vault("palette-routes", true)));
    app.vault_path = Some(std::path::PathBuf::from("palette-routes.anv"));
    let host = app.vault.as_ref().unwrap().hosts()[0].id;
    let key = app.vault.as_ref().unwrap().ssh_keys()[0].id;
    let group = app.vault.as_ref().unwrap().groups()[0].uid.clone();
    let snippet = app.vault.as_ref().unwrap().snippets()[0].id;
    let rule = app.vault.as_ref().unwrap().forwards()[0].id;
    let items = app.palette_items();
    for section in crate::native::Section::MAIN {
        assert!(
            items.iter().any(|item| item.label == section.title()),
            "{}",
            section.title()
        );
    }
    for page in crate::native::settings::Page::ALL {
        assert!(
            items.iter().any(|item| item.label == page.title()),
            "{}",
            page.title()
        );
    }

    app.run_action(Action::EditHost(host));
    assert_eq!((app.active, app.section), (0, crate::native::Section::Hosts));
    assert!(app.hosts.editing(host));

    app.run_action(Action::EditKey(key));
    assert_eq!((app.active, app.section), (0, crate::native::Section::Keychain));
    assert!(app.keychain.editing(key));

    app.run_action(Action::EditGroup(group));
    assert_eq!(app.section, crate::native::Section::Hosts);
    assert!(app.hosts.group_editor.is_some());

    app.run_action(Action::EditSnippet(snippet));
    assert_eq!(app.section, crate::native::Section::Snippets);
    assert!(app.snippets.editing(snippet));

    app.run_action(Action::EditForward(rule));
    assert_eq!(app.section, crate::native::Section::PortForwarding);
    assert!(app.forwards.editing(rule));

    app.run_action(Action::Settings(crate::native::settings::Page::Shortcuts));
    assert_eq!(app.section, crate::native::Section::Settings);
    assert_eq!(app.settings_view.page, crate::native::settings::Page::Shortcuts);
}

#[test]
fn the_fixed_tabs_cannot_be_closed_and_closing_moves_focus_left() {
    let mut app = NativeApp::new(None);
    replay(&mut app, "a");
    replay(&mut app, "b");
    app.close_tab(0);
    app.close_tab(1);
    assert_eq!(titles(&app), ["Vault", "SFTP", "a", "b"]);
    app.close_tab(3);
    assert_eq!((app.tabs.len(), app.active), (3, 2));
    // The last session gone, the vault is in front, not the SFTP picker.
    app.close_tab(2);
    assert_eq!((titles(&app), app.active), (vec!["Vault", "SFTP"], 0));
}

#[test]
fn the_tabs_x_stops_its_live_session_before_removing_the_tab() {
    let mut app = NativeApp::new(Some(fixture_vault("tab-x-closes-session", true)));
    let mut terminal = crate::native::live::LiveTerminal::held_at(Program::DefaultShell, 0, &[]);
    let closed = terminal.watch_close();
    app.push_terminal(
        "live",
        TermView::Live(Box::new(crate::native::panes::Panes::new(terminal))),
    );
    let mut state = UiState::default();
    let output = step(&mut app, &mut state, &[], 0.0);
    step(&mut app, &mut state, &click_on(&output, "Close tab"), 1.0);
    assert!(
        closed.load(std::sync::atomic::Ordering::SeqCst),
        "the X must stop the live backend before it drops the tab"
    );
    assert_eq!(app.tabs.len(), crate::native::FIXED_TABS);
}

#[test]
fn closing_a_background_tab_keeps_the_front_tab() {
    let mut app = NativeApp::new(None);
    replay(&mut app, "a");
    replay(&mut app, "b");
    app.active = 3;
    app.close_tab(2);
    assert_eq!(app.tabs[app.active].title, "b");
}

#[test]
fn a_log_is_deleted_only_after_a_yes() {
    let mut app = NativeApp::new(Some(fixture_vault("delete-log", true)));
    app.section = crate::native::Section::Logs;
    let logs = |app: &NativeApp| app.vault.as_ref().unwrap().logs().len();
    assert_eq!(logs(&app), 2);
    let mut state = UiState::default();
    let output = step(&mut app, &mut state, &[], 0.0);
    let output = step(
        &mut app,
        &mut state,
        &right_click_on(&output, "prod-db"),
        1.0,
    );
    let output = step(&mut app, &mut state, &click_on(&output, "Delete"), 2.0);
    assert_eq!(logs(&app), 2, "asked first, nothing gone yet");
    // Cancel keeps it.
    let output = step(&mut app, &mut state, &click_on(&output, "Cancel"), 3.0);
    assert_eq!(logs(&app), 2);
    // From its row's button this time, and yes.
    let output = step(
        &mut app,
        &mut state,
        &click_within(&output, "prod-db", "Delete"),
        4.0,
    );
    step(&mut app, &mut state, &click_on(&output, "Delete log"), 5.0);
    assert_eq!(logs(&app), 1);
    assert!(
        app.vault
            .as_ref()
            .unwrap()
            .logs()
            .iter()
            .all(|log| log.title != "prod-db")
    );
}

#[test]
fn a_shortcut_takes_new_keys_in_settings_and_its_own_again_on_reset() {
    let mut app = NativeApp::new(Some(fixture_vault("rebind", false)));
    app.section = crate::native::Section::Settings;
    let mut state = UiState::default();
    let press = |key, modifiers| vec![Event::Key(key, modifiers)];
    let ctrl_alt = Modifiers {
        ctrl: true,
        alt: true,
        ..Modifiers::NONE
    };
    let output = step(&mut app, &mut state, &[], 0.0);
    let output = step(&mut app, &mut state, &click_on(&output, "Shortcuts"), 1.0);
    // Escape stops recording and changes nothing.
    step(
        &mut app,
        &mut state,
        &click_on(&output, "Change the keys of Local terminal"),
        2.0,
    );
    let output = step(
        &mut app,
        &mut state,
        &press(Key::Escape, Modifiers::NONE),
        3.0,
    );
    assert!(
        output
            .zones
            .iter()
            .all(|zone| !zone.label.starts_with("Recording"))
    );
    step(
        &mut app,
        &mut state,
        &click_on(&output, "Change the keys of New tab"),
        4.0,
    );
    // Keys another shortcut has are refused, and reach that one no more than any other.
    step(&mut app, &mut state, &[chord(shortcuts::PALETTE)], 5.0);
    assert!(app.palette.is_none(), "the palette's keys opened nothing");
    assert_eq!(app.settings.keys, []);
    let tabs = app.tabs.len();
    let output = step(&mut app, &mut state, &press(Key::Char('n'), ctrl_alt), 6.0);
    assert_eq!(app.tabs.len(), tabs, "recorded, not acted on");
    assert_eq!(
        app.settings.keys,
        vec![KeyBinding {
            action: "NEW_TAB".into(),
            keys: shortcuts::chord_text(Key::Char('n'), ctrl_alt),
        }]
    );
    let new_tab = chord(shortcuts::NEW_TAB);
    let reset = format!("Reset New tab to {}", own_keys(shortcuts::NEW_TAB));
    assert!(output.zones.iter().any(|zone| zone.label == reset));
    // The old keys open nothing now; the new ones open a tab.
    step(&mut app, &mut state, std::slice::from_ref(&new_tab), 7.0);
    assert_eq!(app.tabs.len(), tabs);
    step(&mut app, &mut state, &press(Key::Char('n'), ctrl_alt), 8.0);
    assert_eq!(app.tabs.len(), tabs + 1);
    // Back in Settings, Reset gives it its own keys again.
    app.active = 0;
    let output = step(&mut app, &mut state, &[], 9.0);
    step(&mut app, &mut state, &click_on(&output, &reset), 10.0);
    assert_eq!(app.settings.keys, []);
    step(&mut app, &mut state, &[new_tab], 11.0);
    assert_eq!(app.tabs.len(), tabs + 2);
}

#[test]
fn the_menu_bars_items_do_what_their_shortcuts_do_and_nothing_while_locked() {
    use crate::native::Section;
    use crate::native::settings::Page;
    use annulus_platform::window::MenuItem;
    let mut app = NativeApp::new(Some(fixture_vault("menu-bar", false)));
    let mut state = UiState::default();
    step(&mut app, &mut state, &[], 0.0);
    app.active = 1;
    app.menu(MenuItem::About);
    assert_eq!(
        (app.active, app.section, app.settings_view.page),
        (0, Section::Settings, Page::About)
    );
    app.section = Section::Hosts;
    app.menu(MenuItem::Settings);
    assert_eq!(app.section, Section::Settings);
    app.menu(MenuItem::Lock);
    assert!(app.vault.is_none(), "locked");
    app.section = Section::Hosts;
    app.menu(MenuItem::Settings);
    assert_eq!(app.section, Section::Hosts, "Settings waits for an unlock");
}

#[test]
fn the_menu_bar_shows_the_shortcuts_keys_but_none_while_settings_records_keys() {
    use annulus_platform::window::MenuItem;
    let mut app = NativeApp::new(Some(fixture_vault("menu-keys", false)));
    app.section = crate::native::Section::Settings;
    let mut state = UiState::default();
    let output = step(&mut app, &mut state, &[], 0.0);
    assert_eq!(app.menu_keys(MenuItem::About), None);
    assert_eq!(app.menu_keys(MenuItem::Settings), shortcuts::SETTINGS.own());
    assert_eq!(app.menu_keys(MenuItem::Lock), shortcuts::LOCK.own());
    // Keys chosen in Settings show in the menu bar too.
    let ctrl_alt_k = (
        Key::Char('k'),
        Modifiers {
            ctrl: true,
            alt: true,
            ..Modifiers::NONE
        },
    );
    app.settings.keys.push(KeyBinding {
        action: "LOCK".into(),
        keys: shortcuts::chord_text(ctrl_alt_k.0, ctrl_alt_k.1),
    });
    let output = step(&mut app, &mut state, &click_on(&output, "Shortcuts"), 1.0);
    assert_eq!(app.menu_keys(MenuItem::Lock), Some(ctrl_alt_k));
    // While keys are recorded, the menu bar takes none of them.
    step(
        &mut app,
        &mut state,
        &click_on(&output, "Change the keys of New tab"),
        2.0,
    );
    assert_eq!(app.menu_keys(MenuItem::Settings), None);
    assert_eq!(app.menu_keys(MenuItem::Lock), None);
    app.settings.keys.clear();
    step(&mut app, &mut state, &[], 3.0);
}

#[test]
fn a_reset_that_would_share_keys_is_refused_and_reset_all_puts_everything_back() {
    let mut app = NativeApp::new(Some(fixture_vault("rebind-clash", false)));
    app.section = crate::native::Section::Settings;
    let mut state = UiState::default();
    let press = |key, modifiers| vec![Event::Key(key, modifiers)];
    let ctrl_alt = Modifiers {
        ctrl: true,
        alt: true,
        ..Modifiers::NONE
    };
    let output = step(&mut app, &mut state, &[], 0.0);
    step(&mut app, &mut state, &click_on(&output, "Shortcuts"), 1.0);
    // New tab moves away and Local terminal takes its old keys: New tab's Reset would make
    // the two share them.
    let mut time = 2.0;
    for (name, keys) in [
        ("Change the keys of New tab", (Key::Char('n'), ctrl_alt)),
        (
            "Change the keys of Local terminal",
            shortcuts::NEW_TAB.own().unwrap(),
        ),
    ] {
        let output = step(&mut app, &mut state, &[], time);
        step(&mut app, &mut state, &click_on(&output, name), time + 0.5);
        step(&mut app, &mut state, &press(keys.0, keys.1), time + 1.0);
        time += 2.0;
    }
    assert_eq!(app.settings.keys.len(), 2);
    let output = step(&mut app, &mut state, &[], time);
    let reset = format!("Reset New tab to {}", own_keys(shortcuts::NEW_TAB));
    step(&mut app, &mut state, &click_on(&output, &reset), time + 1.0);
    assert_eq!(app.settings.keys.len(), 2, "no reset into a clash");
    assert_eq!(
        shortcuts::NEW_TAB.keys(),
        shortcuts::chord_text(Key::Char('n'), ctrl_alt)
    );
    let output = step(&mut app, &mut state, &[], time + 2.0);
    step(
        &mut app,
        &mut state,
        &click_on(&output, "Reset all"),
        time + 3.0,
    );
    assert_eq!(app.settings.keys, []);
    assert_eq!(
        (shortcuts::NEW_TAB.keys(), shortcuts::LOCAL.keys()),
        (own_keys(shortcuts::NEW_TAB), own_keys(shortcuts::LOCAL))
    );
}

/// Lays out like [`Measure`] and keeps each text as drawn: fitted to its place, so with
/// its ellipsis where it was cut.
struct Words(TextEngine, Vec<String>);

impl Canvas for Words {
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
        text: &str,
        _: annulus_ui::gfx::Font,
        _: annulus_ui::gfx::Color,
    ) {
        self.1.push(text.to_owned());
    }
    fn push_clip(&mut self, _: Rect) {}
    fn pop_clip(&mut self) {}
}

#[test]
fn squeezed_tabs_keep_a_readable_part_of_their_names() {
    let mut app = NativeApp::new(Some(fixture_vault("squeezed-tabs", false)));
    for index in 0..6 {
        replay(&mut app, &format!("session number {index}"));
    }
    let mut state = UiState::default();
    let mut words = Words(TextEngine::new(), Vec::new());
    // The second frame, once the new tabs have grown into place.
    for time in [0.0, 5.0] {
        words.1.clear();
        state.begin_frame(&[], time);
        let viewport = Rect::new(0.0, 0.0, 1100.0, 800.0);
        let mut ui = Ui::new(
            &mut words,
            Theme::new(Appearance::Dark),
            &mut state,
            viewport,
        );
        app.frame(&mut ui);
        state.end_frame();
    }
    // The tabs' titles, whole or cut with an ellipsis.
    let titles: Vec<&str> = words
        .1
        .iter()
        .map(String::as_str)
        .filter(|text| text.starts_with('s'))
        .collect();
    // Five tabs behind the one in front show no close button, so "sessio…" at least.
    let readable = titles
        .iter()
        .filter(|title| title.starts_with("sessio"))
        .count();
    assert!(readable >= 5, "{titles:?}");
}

#[test]
fn many_tabs_share_the_strip_and_none_shrinks_to_nothing() {
    let mut app = NativeApp::new(Some(fixture_vault("many-tabs", false)));
    for index in 0..6 {
        replay(&mut app, &format!("session number {index}"));
    }
    let mut state = UiState::default();
    let output = step(&mut app, &mut state, &[], 0.0);
    let tabs: Vec<&Zone> = output
        .zones
        .iter()
        .filter(|zone| zone.role == Role::Tab && zone.label.starts_with("session"))
        .collect();
    assert_eq!(tabs.len(), 6, "every tab fits in a 1100-wide window");
    let widths: Vec<f32> = tabs.iter().map(|zone| zone.rect.w).collect();
    assert!(
        widths.iter().all(|width| *width >= 96.0 - 0.5),
        "{widths:?}"
    );
    assert!(
        widths
            .windows(2)
            .all(|pair| (pair[0] - pair[1]).abs() < 1.0),
        "they share it evenly: {widths:?}"
    );
}

#[test]
fn tabs_past_the_strips_end_wait_in_the_all_tabs_menu_and_come_back_when_picked() {
    let mut app = NativeApp::new(Some(fixture_vault("all-tabs", false)));
    let mut state = UiState::default();
    let output = step(&mut app, &mut state, &[], 0.0);
    assert!(
        !output.zones.iter().any(|zone| zone.label == "All tabs"),
        "no menu while every tab fits"
    );
    for index in 0..16 {
        replay(&mut app, &format!("session {index:02}"));
    }
    app.active = 2;
    let output = step(&mut app, &mut state, &[], 1.0);
    let on_strip = |output: &annulus_ui::kit::FrameOutput| -> Vec<String> {
        output
            .zones
            .iter()
            .filter(|zone| zone.role == Role::Tab && zone.label.starts_with("session"))
            .map(|zone| zone.label.clone())
            .collect()
    };
    assert!(
        on_strip(&output).len() < 16,
        "not every tab fits: {:?}",
        on_strip(&output)
    );
    assert!(!on_strip(&output).contains(&"session 15".to_owned()));
    let output = step(&mut app, &mut state, &click_on(&output, "All tabs"), 2.0);
    let listed = output
        .zones
        .iter()
        .filter(|zone| zone.role == Role::MenuItem)
        .count();
    assert!(listed > 0, "the menu lists the tabs");
    step(&mut app, &mut state, &click_on(&output, "session 15"), 3.0);
    assert_eq!(
        app.tabs[app.active].title, "session 15",
        "picked, it is in front"
    );
    let output = step(&mut app, &mut state, &[], 4.0);
    assert!(
        on_strip(&output).contains(&"session 15".to_owned()),
        "and on the strip: {:?}",
        on_strip(&output)
    );
    assert_eq!(app.tabs.len(), crate::native::FIXED_TABS + 16, "no tab lost");
}

/// A few thousand random steps on every push; `ANNULUS_MONKEY_STEPS` and `ANNULUS_MONKEY_SEED`
/// make a run longer and replay a failure.
#[test]
fn random_input_finds_no_panic_or_fault() {
    // Its users run on every core, like the scene renders: never beside a timed shell.
    let _alone = TIMED_SHELLS
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let steps = std::env::var("ANNULUS_MONKEY_STEPS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1500);
    let seeds: Vec<u64> = match std::env::var("ANNULUS_MONKEY_SEED") {
        Ok(seed) => vec![seed.parse().expect("ANNULUS_MONKEY_SEED is a number")],
        Err(_) => vec![1, 2, 3],
    };
    // Each run has its own app and vault: they run side by side.
    std::thread::scope(|scope| {
        for seed in seeds {
            scope.spawn(move || random_user(seed, steps, (1100.0, 760.0)));
            // The smallest window the app allows, where layouts break first.
            scope.spawn(move || random_user(seed + 1000, steps / 2, (640.0, 420.0)));
        }
    });
}

/// A click on the zone named `name` that lies inside the zone named `within` (a button of
/// one list row among many with the same buttons).
fn click_within(output: &FrameOutput, within: &str, name: &str) -> Vec<Event> {
    let outer = output
        .zones
        .iter()
        .find(|zone| zone.label == within)
        .unwrap_or_else(|| panic!("no zone named {within:?}"))
        .rect;
    let inner = output
        .zones
        .iter()
        .find(|zone| {
            zone.label == name
                && zone.rect.x >= outer.x
                && zone.rect.y >= outer.y
                && zone.rect.r() <= outer.r()
                && zone.rect.b() <= outer.b()
        })
        .unwrap_or_else(|| panic!("no {name:?} inside {within:?}"));
    vec![
        Event::PointerMoved(inner.rect.center()),
        Event::PointerDown(Button::Primary),
        Event::PointerUp(Button::Primary),
    ]
}

/// A click on the zone named `name` in the last frame.
fn click_on(output: &FrameOutput, name: &str) -> Vec<Event> {
    vec![
        Event::PointerMoved(zone_center(&output.zones, name)),
        Event::PointerDown(Button::Primary),
        Event::PointerUp(Button::Primary),
    ]
}

/// A right click on the zone named `name` in the last frame.
fn right_click_on(output: &FrameOutput, name: &str) -> Vec<Event> {
    vec![
        Event::PointerMoved(zone_center(&output.zones, name)),
        Event::PointerDown(Button::Secondary),
        Event::PointerUp(Button::Secondary),
    ]
}

#[test]
fn a_hosts_context_menu_copies_its_ssh_command_and_duplicates_it() {
    let mut app = NativeApp::new(Some(fixture_vault("more-menu", true)));
    let mut state = UiState::default();
    let mut output = step(&mut app, &mut state, &[], 0.0);
    output = step(
        &mut app,
        &mut state,
        &right_click_on(&output, "prod-db"),
        2.0,
    );
    output = step(
        &mut app,
        &mut state,
        &click_on(&output, "Copy ssh command"),
        3.0,
    );
    let copied = state.take_clipboard().unwrap();
    assert!(
        copied.starts_with("ssh ") && copied.contains("10.0.4.12"),
        "{copied}"
    );
    output = step(
        &mut app,
        &mut state,
        &right_click_on(&output, "prod-db"),
        4.0,
    );
    step(&mut app, &mut state, &click_on(&output, "Duplicate"), 5.0);
    let vault = app.vault.as_ref().unwrap();
    let original = vault
        .hosts()
        .iter()
        .find(|host| host.label == "prod-db")
        .unwrap();
    let copy = vault
        .hosts()
        .iter()
        .find(|host| host.label == "prod-db copy")
        .unwrap();
    assert_ne!(copy.id, original.id);
    assert_eq!(
        (&copy.address, copy.port),
        (&original.address, original.port)
    );
}

#[test]
fn a_hovered_card_shows_only_edit_and_its_other_actions_are_in_its_context_menu() {
    let mut app = NativeApp::new(Some(fixture_vault("card-hover", true)));
    let mut state = UiState::default();
    let mut output = step(&mut app, &mut state, &[], 0.0);
    let card = zone_center(&output.zones, "prod-db");
    output = step(&mut app, &mut state, &[Event::PointerMoved(card)], 1.0);
    let card_rect = output
        .zones
        .iter()
        .find(|zone| zone.label == "prod-db")
        .unwrap()
        .rect;
    let inside: Vec<&str> = output
        .zones
        .iter()
        .filter(|zone| zone.label != "prod-db" && card_rect.overlaps(&zone.rect))
        .map(|zone| zone.label.as_str())
        .collect();
    assert_eq!(inside, ["Edit"], "the card's own buttons");
    // Shift+F10 on the focused card opens the same menu as a right click.
    state.set_focus(
        output
            .zones
            .iter()
            .find(|zone| zone.label == "prod-db")
            .map(|zone| zone.id),
        true,
    );
    output = step(
        &mut app,
        &mut state,
        &[Event::Key(Key::F(10), Modifiers::SHIFT)],
        2.0,
    );
    let menu: Vec<&str> = output
        .zones
        .iter()
        .filter(|zone| zone.role == Role::MenuItem)
        .map(|zone| zone.label.as_str())
        .collect();
    assert_eq!(
        &menu[..5],
        [
            "Connect",
            "Open SFTP",
            "Edit",
            "Copy ssh command",
            "Duplicate"
        ]
    );
    let _ = step(&mut app, &mut state, &click_on(&output, "Edit"), 3.0);
    assert!(
        app.hosts.editor.is_some(),
        "Edit from the menu opens the editor"
    );
    // It slides in; a moment later it is all there.
    output = step(&mut app, &mut state, &[], 4.0);
    assert!(output.zones.iter().any(|zone| zone.label == "Label"));
}

#[test]
fn a_hovered_cards_context_menu_stays_open_after_the_hover_fades() {
    let mut app = NativeApp::new(Some(fixture_vault("more-stays", true)));
    let mut state = UiState::default();
    let mut output = step(&mut app, &mut state, &[], 0.0);
    let on_card = [Event::PointerMoved(zone_center(&output.zones, "prod-db"))];
    output = step(&mut app, &mut state, &on_card, 1.0);
    output = step(
        &mut app,
        &mut state,
        &right_click_on(&output, "prod-db"),
        2.0,
    );
    // The menu's backdrop now covers the card; well past any fade, the menu is still there.
    for time in [2.5, 3.0, 4.0] {
        output = step(&mut app, &mut state, &[], time);
    }
    assert!(
        output.zones.iter().any(|zone| zone.label == "Duplicate"),
        "the menu closed itself"
    );
}

/// The uid of the group named `name`.
fn group_uid(app: &NativeApp, name: &str) -> String {
    let vault = app.vault.as_ref().unwrap();
    let group = vault.groups().iter().find(|group| group.name == name);
    group.unwrap().uid.clone()
}

/// The saved host labelled `label`.
fn host_named<'a>(app: &'a NativeApp, label: &str) -> &'a Host {
    let vault = app.vault.as_ref().unwrap();
    vault
        .hosts()
        .iter()
        .find(|host| host.label == label)
        .unwrap()
}

/// The tab queued last: its title, who it logs in as where, the password it brings, and the
/// saved host it came from.
fn queued(app: &NativeApp) -> (String, String, Option<Password>, Option<OpenTab>) {
    let (title, program, origin) = app.opens.last().unwrap();
    let Program::Ssh {
        target, password, ..
    } = program
    else {
        panic!("an ssh program")
    };
    let login = format!("{}@{}:{}", target.user, target.host, target.port);
    (title.clone(), login, password.clone(), origin.clone())
}

#[test]
fn connect_all_opens_every_host_directly_in_the_group() {
    let mut app = NativeApp::new(Some(fixture_vault("connect-all", true)));
    let production = group_uid(&app, "Production");
    // A host of a group inside Production is not directly in it.
    let vault = app.vault.as_mut().unwrap();
    let mut inner = vault.draft_group(Some(production)).unwrap();
    inner.name = "Inner".into();
    vault.upsert_group(inner.clone()).unwrap();
    let mut deep = vault.draft_host();
    (deep.label, deep.address, deep.group) = ("deep".into(), "deep.lan".into(), Some(inner.uid));
    vault.upsert_host(deep).unwrap();
    let mut state = UiState::default();
    let mut output = step(&mut app, &mut state, &[], 0.0);
    output = step(&mut app, &mut state, &click_on(&output, "Production"), 1.0);
    // The tabs open at the end of the click's own frame.
    drive(&mut app, &mut state, &click_on(&output, "Connect all"), 2.0);
    let opened: Vec<(&str, Option<OpenTab>)> = app
        .tabs
        .iter()
        .skip(crate::native::FIXED_TABS)
        .map(|tab| (tab.title.as_str(), tab.origin.clone()))
        .collect();
    let expected: Vec<(&str, Option<OpenTab>)> = ["api-1", "api-2", "worker-1"]
        .into_iter()
        .map(|label| (label, Some(OpenTab::Host(host_named(&app, label).id))))
        .collect();
    assert_eq!(opened, expected);
}

#[test]
fn the_tag_filter_works_by_keyboard() {
    let mut app = NativeApp::new(Some(fixture_vault("tags-keyboard", true)));
    let mut state = UiState::default();
    let mut output = step(&mut app, &mut state, &[], 0.0);
    output = step(&mut app, &mut state, &click_on(&output, "Tags"), 1.0);
    // Tab goes on from the Tags button to the chips.
    let chip = |output: &FrameOutput| {
        let zone = output.zones.iter().find(|zone| zone.label == "nginx (2)");
        zone.unwrap().id
    };
    let mut time = 2.0;
    while state.focus() != Some(chip(&output)) {
        assert!(time < 40.0, "Tab never reached the chip");
        output = step(
            &mut app,
            &mut state,
            &[Event::Key(Key::Tab, Modifiers::NONE)],
            time,
        );
        time += 1.0;
    }
    let space = [Event::Key(Key::Space, Modifiers::NONE)];
    output = step(&mut app, &mut state, &space, time);
    assert_eq!(app.hosts.tags, ["nginx"]);
    let listed = |label: &str| output.zones.iter().any(|zone| zone.label == label);
    assert!(listed("web-1") && listed("staging-web"), "from every group");
    assert!(!listed("prod-db"));
    // Clear drops the filter; so does closing the chips.
    output = step(
        &mut app,
        &mut state,
        &click_on(&output, "Clear"),
        time + 1.0,
    );
    assert_eq!(app.hosts.tags, Vec::<String>::new());
    output = step(
        &mut app,
        &mut state,
        &click_on(&output, "nginx (2)"),
        time + 2.0,
    );
    assert_eq!(app.hosts.tags, ["nginx"]);
    output = step(&mut app, &mut state, &click_on(&output, "Tags"), time + 3.0);
    assert_eq!(app.hosts.tags, Vec::<String>::new());
    assert!(!output.zones.iter().any(|zone| zone.label == "nginx (2)"));
}

#[test]
fn move_to_group_puts_the_host_in_the_chosen_group_or_back_on_top() {
    let mut app = NativeApp::new(Some(fixture_vault("move-to", true)));
    let production = group_uid(&app, "Production");
    let mut state = UiState::default();
    let mut output = step(&mut app, &mut state, &[], 0.0);
    output = step(&mut app, &mut state, &click_on(&output, "prod-db"), 1.0);
    let more = right_click_on(&output, "prod-db");
    output = step(&mut app, &mut state, &more, 2.0);
    let menu = click_on(&output, "Move to Production");
    output = step(&mut app, &mut state, &menu, 3.0);
    assert_eq!(
        host_named(&app, "prod-db").group.as_deref(),
        Some(production.as_str())
    );
    // It left the top level: open Production to find it, then move it back up.
    output = step(&mut app, &mut state, &click_on(&output, "Production"), 4.0);
    output = step(&mut app, &mut state, &click_on(&output, "prod-db"), 5.0);
    let more = right_click_on(&output, "prod-db");
    output = step(&mut app, &mut state, &more, 6.0);
    let menu = click_on(&output, "Move to the top level");
    step(&mut app, &mut state, &menu, 7.0);
    assert_eq!(host_named(&app, "prod-db").group, None);
}

#[test]
fn duplicate_saves_a_copy_with_a_new_id_and_the_same_address() {
    // From a row of the list, whose context menu is its own.
    let mut app = NativeApp::new(Some(fixture_vault("duplicate", true)));
    app.hosts.mode = Mode::List;
    let before = app.vault.as_ref().unwrap().hosts().len();
    let mut state = UiState::default();
    let mut output = step(&mut app, &mut state, &[], 0.0);
    let hover = [Event::PointerMoved(zone_center(&output.zones, "grafana"))];
    output = step(&mut app, &mut state, &hover, 1.0);
    let more = right_click_on(&output, "grafana");
    output = step(&mut app, &mut state, &more, 2.0);
    step(&mut app, &mut state, &click_on(&output, "Duplicate"), 3.0);
    assert_eq!(app.vault.as_ref().unwrap().hosts().len(), before + 1);
    let (original, copy) = (
        host_named(&app, "grafana"),
        host_named(&app, "grafana copy"),
    );
    assert_ne!(copy.id, original.id);
    assert_eq!(
        (&copy.address, &copy.username, copy.port),
        (&original.address, &original.username, original.port)
    );
}

#[test]
fn the_host_editor_duplicates_what_it_shows_and_opens_the_copy() {
    let mut app = NativeApp::new(Some(fixture_vault("duplicate-in-editor", true)));
    let before = app.vault.as_ref().unwrap().hosts().len();
    let original = host_named(&app, "grafana").clone();
    let mut state = UiState::default();
    step(&mut app, &mut state, &[], 0.0);
    app.hosts
        .edit_host(app.vault.as_ref().unwrap(), original.id);
    let mut output = step(&mut app, &mut state, &[], 1.0);
    // A port typed and not saved goes with the copy; the host keeps what was saved.
    let mut events = click_on(&output, "Port");
    events.push(Event::Key(Key::End, Modifiers::NONE));
    events.push(Event::Text("9".into()));
    output = step(&mut app, &mut state, &events, 2.0);
    step(&mut app, &mut state, &click_on(&output, "Duplicate"), 3.0);
    assert_eq!(app.vault.as_ref().unwrap().hosts().len(), before + 1);
    let copy = host_named(&app, "grafana copy").clone();
    let typed = format!(
        "{}9",
        original
            .port
            .map(|port| port.to_string())
            .unwrap_or_default()
    );
    assert_eq!(copy.port, typed.parse().ok());
    assert_eq!(
        (&copy.address, &copy.username),
        (&original.address, &original.username)
    );
    assert_eq!(
        host_named(&app, "grafana"),
        &original,
        "the host is as it was saved"
    );
    // The editor goes on with the copy.
    assert_eq!(app.hosts.selected, Some(copy.id));
    assert!(app.hosts.editor.is_some());
    let output = step(&mut app, &mut state, &[], 4.0);
    assert!(
        output.zones.iter().any(|zone| zone.label == "grafana copy"),
        "the copy is in the list"
    );
}

#[test]
fn a_click_on_a_host_selects_it_or_with_one_click_connecting_connects() {
    let mut app = NativeApp::new(Some(fixture_vault("one-click", true)));
    let mut state = UiState::default();
    let mut output = step(&mut app, &mut state, &[], 0.0);
    let tabs = app.tabs.len();
    output = step(&mut app, &mut state, &click_on(&output, "grafana"), 1.0);
    assert_eq!(app.tabs.len(), tabs, "off: nothing opens");
    assert_eq!(app.hosts.selected, Some(host_named(&app, "grafana").id));
    app.settings.connect_on_click = true;
    step(&mut app, &mut state, &click_on(&output, "nas"), 2.0);
    assert_eq!(app.tabs.len(), tabs + 1, "on: one click connects");
    assert_eq!(app.tabs[app.active].title, "nas");
}

#[test]
fn the_theme_tabs_offer_schemes_of_their_kind_and_drop_one_of_the_other() {
    let mut app = NativeApp::new(Some(fixture_vault("theme-tabs", true)));
    settings(&mut app);
    app.settings.app_scheme = Some("Dracula".into());
    let mut state = UiState::default();
    let mut output = step(&mut app, &mut state, &[], 0.0);
    output = step(&mut app, &mut state, &click_on(&output, "Appearance"), 1.0);
    output = step(&mut app, &mut state, &click_on(&output, "Light"), 2.0);
    assert_eq!(app.settings.theme, ThemeMode::Light);
    assert_eq!(app.settings.app_scheme, None, "the dark scheme went");
    let listed = |app: &mut NativeApp, state: &mut UiState, output: &FrameOutput, at: f64| {
        let menu = output
            .zones
            .iter()
            .find(|zone| zone.role == Role::Button && zone.label == "Annulus")
            .map(|zone| zone.rect.center())
            .unwrap();
        let events = [
            Event::PointerMoved(menu),
            Event::PointerDown(Button::Primary),
            Event::PointerUp(Button::Primary),
        ];
        step(app, state, &events, at);
        let output = step(app, state, &[Event::Text("solarized".into())], at + 0.5);
        let names: Vec<String> = output
            .zones
            .iter()
            .filter(|zone| zone.role == Role::ListItem)
            .map(|zone| zone.label.clone())
            .collect();
        step(
            app,
            state,
            &[Event::Key(Key::Escape, Modifiers::NONE)],
            at + 1.0,
        );
        step(app, state, &[], at + 1.5);
        names
    };
    // Whether each scheme found is dark.
    let dark = |names: Vec<String>| -> Vec<bool> {
        names
            .iter()
            .map(|name| annulus_config::schemes::Scheme::find(name).unwrap().dark())
            .collect()
    };
    let light_mode = dark(listed(&mut app, &mut state, &output, 3.0));
    assert!(!light_mode.is_empty() && light_mode.iter().all(|dark| !dark));
    output = step(&mut app, &mut state, &click_on(&output, "Dark"), 5.0);
    let dark_mode = dark(listed(&mut app, &mut state, &output, 6.0));
    assert!(!dark_mode.is_empty() && dark_mode.iter().all(|dark| *dark));
    output = step(&mut app, &mut state, &click_on(&output, "System"), 8.0);
    let system = dark(listed(&mut app, &mut state, &output, 9.0));
    assert!(
        system.contains(&true) && system.contains(&false),
        "every scheme"
    );
}

#[test]
fn a_typed_target_can_be_saved_as_a_host() {
    let mut app = NativeApp::new(Some(fixture_vault("save-target", true)));
    let before = app.vault.as_ref().unwrap().hosts().len();
    let search = "Search hosts, or type user@host:port to connect";
    let mut state = UiState::default();
    let mut output = step(&mut app, &mut state, &[], 0.0);
    let mut events = click_on(&output, search);
    events.push(Event::Text("deploy@10.0.9.9:2200".into()));
    output = step(&mut app, &mut state, &events, 1.0);
    let _ = step(
        &mut app,
        &mut state,
        &click_on(&output, "Save as host"),
        2.0,
    );
    // The editor slides in on it, filled in; Save keeps it.
    output = step(&mut app, &mut state, &[], 2.5);
    let _ = step(&mut app, &mut state, &click_on(&output, "Save"), 3.0);
    assert_eq!(app.vault.as_ref().unwrap().hosts().len(), before + 1);
    let saved = host_named(&app, "10.0.9.9");
    assert_eq!(
        (saved.address.as_str(), saved.username.as_str(), saved.port),
        ("10.0.9.9", "deploy", Some(2200))
    );
    // Words that match nothing are no target: nothing to save.
    for (at, typed) in [(4.0, "just words"), (5.0, "zzz")] {
        let mut events = click_on(&output, search);
        events.extend([
            Event::Key(Key::Char('a'), Modifiers::COMMAND),
            Event::Key(Key::Backspace, Modifiers::NONE),
            Event::Text(typed.into()),
        ]);
        output = step(&mut app, &mut state, &events, at);
        assert!(
            !output.zones.iter().any(|zone| zone.label == "Save as host"),
            "{typed} was offered"
        );
    }
}

#[test]
fn quick_connect_to_a_saved_label_uses_its_details() {
    let mut app = NativeApp::new(Some(fixture_vault("quick-saved", true)));
    let id = host_named(&app, "prod-db").id;
    app.act(HostsAction::QuickConnect("PROD-DB".into()));
    let (title, login, _, origin) = queued(&app);
    assert_eq!(
        (title.as_str(), login.as_str(), origin),
        ("prod-db", "postgres@10.0.4.12:22", Some(OpenTab::Host(id)))
    );
}

#[test]
fn a_saved_password_goes_with_the_saved_host_only() {
    let mut app = NativeApp::new(Some(fixture_vault("quick-password", true)));
    let mut host = host_named(&app, "prod-db").clone();
    host.password = Some(annulus_vault::hosts::Secret::new("hunter2"));
    app.vault.as_mut().unwrap().upsert_host(host).unwrap();
    app.act(HostsAction::QuickConnect("prod-db".into()));
    assert_eq!(queued(&app).2, Some(Password::new("hunter2")));
    app.act(HostsAction::QuickConnect("postgres@10.0.4.12:22".into()));
    assert_eq!(queued(&app).2, None);
}

#[test]
fn quick_connect_to_anything_else_is_an_ssh_target() {
    let mut app = NativeApp::new(Some(fixture_vault("quick-other", true)));
    app.act(HostsAction::QuickConnect("root@other:22".into()));
    let (title, login, password, origin) = queued(&app);
    assert_eq!(
        (title.as_str(), login.as_str(), password, origin),
        ("root@other:22", "root@other:22", None, None)
    );
}

#[test]
fn the_lock_screens_tree_names_the_password_fields_but_never_their_text() {
    let mut app = NativeApp::new(None);
    app.lock = crate::native::lock::LockView::new(crate::native::lock::Mode::Setup);
    let mut state = UiState::default();
    let mut output = step(&mut app, &mut state, &[], 0.0);
    let mut events = click_on(&output, "Master password (10+ characters)");
    events.push(Event::Text("correct horse battery".into()));
    output = step(&mut app, &mut state, &events, 1.0);
    let tree = annulus_ui::access::tree(&output.zones, state.focus(), 1.0, "Annulus");
    assert_eq!(annulus_ui::access::faults(&tree), Vec::<String>::new());
    let (id, field) = tree
        .nodes
        .iter()
        .find(|(_, node)| node.label() == Some("Master password (10+ characters)"))
        .unwrap();
    assert_eq!(
        field.role(),
        annulus_ui::access::accesskit::Role::PasswordInput
    );
    assert_eq!(tree.focus, *id, "the field typed into has focus");
    assert!(
        tree.nodes
            .iter()
            .all(|(_, node)| node.value().is_none_or(|value| !value.contains("horse"))),
        "the password reached the tree"
    );
}

#[test]
fn the_hosts_listing_sorts_by_its_headers_and_filters_by_name() {
    let mut app = NativeApp::new(Some(fixture_vault("sftp-sort", false)));
    app.push_sftp(crate::native::demo::sftp_tab("sort"));
    let mut state = UiState::default();
    let mut output = step(&mut app, &mut state, &[], 0.0);
    let y = |output: &FrameOutput, name: &str| zone_center(&output.zones, name).y;
    // Folders first, then by name.
    assert!(y(&output, "config") < y(&output, "app.log"));
    assert!(y(&output, "app.log") < y(&output, "deploy.sh"));
    output = step(
        &mut app,
        &mut state,
        &click_on(&output, "Sort by size"),
        1.0,
    );
    assert!(y(&output, "README.md") < y(&output, "deploy.sh"));
    assert!(y(&output, "deploy.sh") < y(&output, "app.log"));
    assert!(
        y(&output, "config") < y(&output, "README.md"),
        "folders stay first"
    );
    output = step(
        &mut app,
        &mut state,
        &click_on(&output, "Sort by size"),
        2.0,
    );
    assert!(
        y(&output, "app.log") < y(&output, "deploy.sh"),
        "again: largest first"
    );
    let mut events = click_on(&output, "Filter this folder");
    events.push(Event::Text("DEPLOY".into()));
    output = step(&mut app, &mut state, &events, 3.0);
    let names: Vec<&str> = output
        .zones
        .iter()
        .map(|zone| zone.label.as_str())
        .collect();
    assert!(
        names.contains(&"deploy.sh") && !names.contains(&"app.log"),
        "{names:?}"
    );
}

#[test]
fn deleting_on_the_host_asks_first() {
    let mut app = NativeApp::new(Some(fixture_vault("sftp-delete", false)));
    app.push_sftp(crate::native::demo::sftp_tab("delete"));
    let mut state = UiState::default();
    let mut output = step(&mut app, &mut state, &[], 0.0);
    let card_up = |output: &FrameOutput| output.zones.iter().any(|zone| zone.label == "Delete");
    output = step(&mut app, &mut state, &click_on(&output, "deploy.sh"), 1.0);
    assert!(!card_up(&output));
    let delete = [Event::Key(Key::Delete, Modifiers::NONE)];
    output = step(&mut app, &mut state, &delete, 2.0);
    assert!(card_up(&output), "the Delete key asks");
    output = step(
        &mut app,
        &mut state,
        &[Event::Key(Key::Escape, Modifiers::NONE)],
        3.0,
    );
    assert!(!card_up(&output), "Escape keeps the file");
    assert!(output.zones.iter().any(|zone| zone.label == "deploy.sh"));
    // The More menu's Delete asks the same.
    output = step(&mut app, &mut state, &click_on(&output, "deploy.sh"), 4.0);
    output = step(
        &mut app,
        &mut state,
        &click_on(&output, "More actions"),
        5.0,
    );
    output = step(&mut app, &mut state, &click_on(&output, "Delete"), 6.0);
    assert!(card_up(&output), "the menu asks too");
    output = step(&mut app, &mut state, &click_on(&output, "Cancel"), 7.0);
    assert!(!card_up(&output));
}

#[test]
fn a_snippet_runs_on_the_hosts_ticked_in_one_tab_of_panes() {
    let mut app = NativeApp::new(Some(fixture_vault("snippet-hosts", true)));
    app.section = crate::native::Section::Snippets;
    let mut state = UiState::default();
    let mut output = step(&mut app, &mut state, &[], 0.0);
    output = step(
        &mut app,
        &mut state,
        &click_on(&output, "Run on hosts"),
        1.0,
    );
    output = step(&mut app, &mut state, &click_on(&output, "prod-db"), 2.0);
    output = step(&mut app, &mut state, &click_on(&output, "bastion"), 3.0);
    assert!(
        output
            .zones
            .iter()
            .any(|zone| zone.label == "Run on 2 hosts")
    );
    // One frame: the tab is added at its end, and its panes start connecting when drawn.
    drive(
        &mut app,
        &mut state,
        &click_on(&output, "Run on 2 hosts"),
        4.0,
    );
    assert_eq!(
        titles(&app),
        ["Vault", "SFTP", "Tail the app log on 2 hosts"]
    );
    let panes = front_panes(&app);
    assert_eq!(panes.len(), 2, "a pane per host, side by side");
    assert_eq!(app.active, 2);
}

#[test]
fn trusting_a_servers_certificate_pins_it_until_the_address_changes() {
    let mut app = NativeApp::new(Some(fixture_vault("pin-certificate", true)));
    settings_account_certificate(&mut app);
    let mut state = UiState::default();
    let output = step(&mut app, &mut state, &[], 0.0);
    step(
        &mut app,
        &mut state,
        &click_on(&output, "Trust this certificate"),
        1.0,
    );
    assert_eq!(app.sync.pin, Some((0..=255).collect::<Vec<u8>>()));
    assert!(app.sync.untrusted.is_none());
    // The certificate stopped a Create account: that, not Sign in, is what to redo.
    let (_, message) = app.sync.message.clone().unwrap_or_default();
    assert!(message.contains("not created yet"), "{message}");
    assert!(message.contains("Create account"), "{message}");
    assert!(!message.contains("Sign in"), "{message}");
    // Another address: the certificate trusted for the old one goes.
    let output = step(&mut app, &mut state, &[], 2.0);
    let mut events = click_on(&output, "Server");
    events.push(Event::Text("1".into()));
    step(&mut app, &mut state, &events, 3.0);
    assert_eq!(app.sync.pin, None);
}

#[test]
fn on_macos_the_title_bar_leaves_the_window_buttons_to_the_os() {
    let mut app = NativeApp::new(Some(fixture_vault("os-window-buttons", true)));
    with_tabs(&mut app);
    let mut state = UiState::default();
    let drag = annulus_ui::kit::Id::new(crate::native::titlebar::DRAG);
    for buttons in [WindowButtons::Ours, WindowButtons::Os] {
        app.window_buttons = buttons;
        let output = step(&mut app, &mut state, &[], 0.0);
        let drawn = output
            .zones
            .iter()
            .filter(|zone| zone.role == Role::WindowButton)
            .count();
        let os = buttons == WindowButtons::Os;
        assert_eq!(drawn, if os { 0 } else { 3 }, "{buttons:?}");
        if os {
            // Nothing in the bar sits where macOS draws its buttons.
            let room = annulus_platform::window::TRAFFIC_LIGHTS_W;
            let crowding: Vec<&str> = output
                .zones
                .iter()
                .filter(|zone| zone.id != drag && zone.rect.b() <= crate::native::TITLE_H)
                .filter(|zone| zone.rect.x < room)
                .map(|zone| zone.label.as_str())
                .collect();
            assert!(
                crowding.is_empty(),
                "under the window buttons: {crowding:?}"
            );
        }
    }
}

#[test]
fn the_lock_screen_takes_the_password_without_a_click() {
    let vault = fixture_vault("type-to-unlock", true);
    let mut app = NativeApp::new(None);
    app.vault_path = Some(vault.path().to_path_buf());
    drop(vault);
    let mut state = UiState::default();
    step(&mut app, &mut state, &[], 0.0);
    let typed = [
        Event::Text("not the password".into()),
        Event::Key(Key::Enter, Modifiers::NONE),
    ];
    step(&mut app, &mut state, &typed, 1.0);
    assert!(
        app.lock.working.is_some() || app.lock.error.is_some(),
        "typing went into the password field and Enter tried it"
    );
}

#[test]
fn the_plus_button_opens_a_new_tab_whose_search_connects_in_its_place() {
    let mut app = NativeApp::new(Some(fixture_vault("plus-hosts", true)));
    replay(&mut app, "build");
    let mut state = UiState::default();
    let output = step(&mut app, &mut state, &[], 0.0);
    assert_eq!(app.active, 2, "a terminal in front");
    let output = step(&mut app, &mut state, &click_on(&output, "New tab"), 1.0);
    assert_eq!(titles(&app), ["Vault", "SFTP", "build", "New Tab"]);
    assert_eq!(app.tabs[app.active].kind, crate::native::TabKind::NewTab);
    // The search has the keyboard at once, and the saved hosts are listed newest first.
    let focused = output
        .zones
        .iter()
        .find(|zone| Some(zone.id) == state.focus());
    assert_eq!(
        focused.map(|zone| zone.role),
        Some(Role::TextInput),
        "the search is focused"
    );
    assert!(output.zones.iter().any(|zone| zone.label == "router"));
    // Typing a saved host's name and Enter connects to it, in the new tab's place.
    let typed = [
        Event::Text("prod-db".into()),
        Event::Key(Key::Enter, Modifiers::NONE),
    ];
    step(&mut app, &mut state, &typed, 2.0);
    step(&mut app, &mut state, &[], 3.0);
    assert_eq!(titles(&app), ["Vault", "SFTP", "build", "prod-db"]);
    assert_eq!(app.active, 3);
    // And it is the first of the recent connections now.
    let prod = host_named(&app, "prod-db").id;
    assert_eq!(app.settings.recent[0].host, prod);
}

#[test]
fn the_sftp_tab_picks_a_host_and_goes_back_to_the_picker() {
    let mut app = NativeApp::new(Some(fixture_vault("sftp-tab", true)));
    let mut state = UiState::default();
    let output = step(&mut app, &mut state, &[], 0.0);
    let output = step(&mut app, &mut state, &click_on(&output, "SFTP"), 1.0);
    assert_eq!(app.active, 1);
    assert!(output.zones.iter().any(|zone| zone.label == "nas"));
    let output = step(&mut app, &mut state, &click_on(&output, "nas"), 2.0);
    assert!(
        app.tabs[1].sftp.is_some(),
        "its files, in the SFTP tab itself"
    );
    assert_eq!(app.tabs.len(), crate::native::FIXED_TABS, "no tab was added");
    step(
        &mut app,
        &mut state,
        &click_on(&output, "Back to the hosts"),
        3.0,
    );
    assert!(app.tabs[1].sftp.is_none());
}

#[test]
fn escape_with_the_palette_open_closes_only_the_palette() {
    let mut app = NativeApp::new(Some(fixture_vault("palette-escape", true)));
    let mut state = UiState::default();
    let output = step(&mut app, &mut state, &[], 0.0);
    step(&mut app, &mut state, &click_on(&output, "New host"), 1.0);
    assert!(app.hosts.editor.is_some());
    let palette = [chord(shortcuts::PALETTE)];
    step(&mut app, &mut state, &palette, 2.0);
    assert!(app.palette.is_some());
    let escape = [Event::Key(Key::Escape, Modifiers::NONE)];
    step(&mut app, &mut state, &escape, 3.0);
    assert!(app.palette.is_none(), "the palette closed");
    assert!(app.hosts.editor.is_some(), "the draft under it stays");
}

#[test]
fn a_named_palette_action_opens_the_saved_host_editor_from_the_keyboard() {
    let mut app = NativeApp::new(Some(fixture_vault("palette-edit-host", true)));
    let host = app.vault.as_ref().unwrap().hosts()[0].clone();
    let mut state = UiState::default();
    step(&mut app, &mut state, &[], 0.0);
    step(&mut app, &mut state, &[chord(shortcuts::PALETTE)], 1.0);
    step(
        &mut app,
        &mut state,
        &[Event::Text(format!("Edit ({})", host.label))],
        2.0,
    );
    step(
        &mut app,
        &mut state,
        &[Event::Key(Key::Enter, Modifiers::NONE)],
        3.0,
    );
    step(&mut app, &mut state, &[], 4.0);
    assert!(app.palette.is_none());
    assert_eq!(app.section, crate::native::Section::Hosts);
    assert!(app.hosts.editing(host.id));
}

#[test]
fn choosing_a_palette_result_repaints_without_the_blurred_backdrop() {
    let mut app = NativeApp::new(Some(fixture_vault("palette-final-frame", true)));
    let mut state = UiState::default();
    drive(&mut app, &mut state, &[], 0.0);
    step(&mut app, &mut state, &[chord(shortcuts::PALETTE)], 1.0);
    drive(
        &mut app,
        &mut state,
        &[Event::Text("Known Hosts".into())],
        2.0,
    );
    drive(&mut app, &mut state, &[], 2.0);
    let closing = drive(
        &mut app,
        &mut state,
        &[Event::Key(Key::Enter, Modifiers::NONE)],
        2.1,
    );
    assert!(closing.repaint, "the close motion needs another frame");
    let closed = drive(&mut app, &mut state, &[], 2.3);
    assert!(app.palette.is_none(), "the chosen page closed the palette");
    assert_eq!(app.section, crate::native::Section::KnownHosts);
    assert!(
        closed.repaint,
        "the blurred closing frame must be followed by one clean frame"
    );
}

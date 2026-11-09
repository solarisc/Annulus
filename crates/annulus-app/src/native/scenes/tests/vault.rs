#[test]
fn cancelling_the_palette_returns_keyboard_focus() {
    let mut app = NativeApp::new(Some(fixture_vault("palette-focus", true)));
    let mut state = UiState::default();
    let output = step(&mut app, &mut state, &[], 0.0);
    step(
        &mut app,
        &mut state,
        &click_on(&output, "Search hosts, or type user@host:port to connect"),
        1.0,
    );
    let before = state.focus();
    assert!(before.is_some());
    step(&mut app, &mut state, &[chord(shortcuts::PALETTE)], 2.0);
    step(
        &mut app,
        &mut state,
        &[Event::Key(Key::Escape, Modifiers::NONE)],
        3.0,
    );
    step(&mut app, &mut state, &[], 4.0);
    assert_eq!(state.focus(), before);
}

#[test]
fn enter_in_an_empty_host_search_connects_nowhere() {
    let mut app = NativeApp::new(Some(fixture_vault("empty-search", true)));
    let mut state = UiState::default();
    let output = step(&mut app, &mut state, &[], 0.0);
    let tabs = app.tabs.len();
    step(
        &mut app,
        &mut state,
        &click_on(&output, "Search hosts, or type user@host:port to connect"),
        1.0,
    );
    let enter = [Event::Key(Key::Enter, Modifiers::NONE)];
    step(&mut app, &mut state, &enter, 2.0);
    step(&mut app, &mut state, &[], 3.0);
    assert_eq!(app.tabs.len(), tabs, "no tab opened");
}

#[test]
fn a_saved_key_opens_to_edit_shows_and_copies_its_private_key_on_asking_and_can_be_replaced() {
    let mut app = NativeApp::new(Some(fixture_vault("key-edit", true)));
    app.section = crate::native::Section::Keychain;
    let saved = app.vault.as_ref().unwrap().ssh_keys()[0].clone();
    let mut state = UiState::default();
    let mut output = step(&mut app, &mut state, &[], 0.0);
    // A click on the key opens it; its private key stays dots until the eye is clicked.
    output = step(&mut app, &mut state, &click_on(&output, "Work laptop"), 1.0);
    assert!(
        output
            .zones
            .iter()
            .any(|zone| zone.label == "Show private key")
    );
    output = step(
        &mut app,
        &mut state,
        &click_on(&output, "Show private key"),
        2.0,
    );
    assert!(
        output
            .zones
            .iter()
            .any(|zone| zone.label == "Hide private key")
    );
    output = step(
        &mut app,
        &mut state,
        &click_on(&output, "Copy private key"),
        3.0,
    );
    assert_eq!(
        state.take_copied(),
        Some((saved.private.expose().to_owned(), true)),
        "copied whole, as a secret"
    );
    // Renamed, it keeps its key.
    let mut events = click_on(&output, "Name, e.g. work laptop");
    events.push(Event::Key(Key::Char('a'), Modifiers::COMMAND));
    events.push(Event::Text("Deploy key".into()));
    output = step(&mut app, &mut state, &events, 4.0);
    step(&mut app, &mut state, &click_on(&output, "Save"), 5.0);
    let renamed = app.vault.as_ref().unwrap().ssh_keys()[0].clone();
    assert_eq!(renamed.label, "Deploy key");
    assert_eq!(
        (renamed.id, &renamed.private, &renamed.public),
        (saved.id, &saved.private, &saved.public)
    );
    // Another key pasted over it replaces it under the same id, so its hosts keep it.
    let other = annulus_ssh::keys::generate(annulus_ssh::keys::KeyKind::Ed25519, "other")
        .expect("Ed25519 keys always generate");
    output = step(&mut app, &mut state, &[], 6.0);
    output = step(&mut app, &mut state, &click_on(&output, "Edit"), 7.0);
    let mut events = click_on(
        &output,
        "Paste a private key (-----BEGIN \u{2026} PRIVATE KEY-----)",
    );
    events.push(Event::Key(Key::Char('a'), Modifiers::COMMAND));
    events.push(Event::Paste(other.private.as_str().to_owned()));
    output = step(&mut app, &mut state, &events, 8.0);
    step(&mut app, &mut state, &click_on(&output, "Save"), 9.0);
    let replaced = app.vault.as_ref().unwrap().ssh_keys()[0].clone();
    assert_eq!(replaced.id, saved.id);
    assert_eq!(replaced.label, "Deploy key");
    assert_eq!(
        (&replaced.public, &replaced.fingerprint),
        (&other.public, &other.fingerprint)
    );
    let said = app.keychain.said().map(|(_, line)| line.clone());
    assert!(
        said.as_deref()
            .is_some_and(|line| line.contains("public key changed")),
        "{said:?}"
    );
}

#[test]
fn escape_closes_the_open_card_and_keeps_what_a_delete_would_remove() {
    let escape = [Event::Key(Key::Escape, Modifiers::NONE)];
    let mut app = NativeApp::new(Some(fixture_vault("escape", true)));
    app.section = crate::native::Section::Keychain;
    let mut state = UiState::default();
    let mut output = step(&mut app, &mut state, &[], 0.0);
    output = step(&mut app, &mut state, &click_on(&output, "Import key"), 1.0);
    assert!(
        output
            .zones
            .iter()
            .any(|zone| zone.label == "Name, e.g. id_ed25519")
    );
    output = step(&mut app, &mut state, &escape, 2.0);
    assert!(
        !output
            .zones
            .iter()
            .any(|zone| zone.label == "Name, e.g. id_ed25519")
    );
    output = step(&mut app, &mut state, &click_on(&output, "Delete"), 3.0);
    assert!(output.zones.iter().any(|zone| zone.label == "Delete key"));
    output = step(&mut app, &mut state, &escape, 4.0);
    assert!(!output.zones.iter().any(|zone| zone.label == "Delete key"));
    assert_eq!(
        app.vault.as_ref().unwrap().ssh_keys().len(),
        1,
        "the key stays"
    );
    // Snippets and Port Forwarding cards too.
    app.section = crate::native::Section::Snippets;
    output = step(&mut app, &mut state, &[], 5.0);
    output = step(&mut app, &mut state, &click_on(&output, "New snippet"), 6.0);
    assert!(
        output
            .zones
            .iter()
            .any(|zone| zone.label == "Label, e.g. Restart nginx")
    );
    output = step(&mut app, &mut state, &escape, 7.0);
    assert!(
        !output
            .zones
            .iter()
            .any(|zone| zone.label == "Label, e.g. Restart nginx")
    );
    app.section = crate::native::Section::PortForwarding;
    output = step(&mut app, &mut state, &[], 8.0);
    output = step(&mut app, &mut state, &click_on(&output, "New rule"), 9.0);
    let wizard = |output: &FrameOutput| {
        output
            .zones
            .iter()
            .any(|zone| zone.label == "Reach a service behind a server")
    };
    assert!(wizard(&output), "New rule opens the wizard");
    output = step(&mut app, &mut state, &escape, 10.0);
    assert!(!wizard(&output));
}

#[test]
fn a_tag_is_deleted_from_every_host_only_after_a_yes() {
    let mut app = NativeApp::new(Some(fixture_vault("untag", true)));
    let tagged = |app: &NativeApp| {
        app.vault
            .as_ref()
            .unwrap()
            .hosts()
            .iter()
            .filter(|host| host.tags.iter().any(|tag| tag == "nginx"))
            .count()
    };
    let before = tagged(&app);
    assert!(before > 0);
    let mut state = UiState::default();
    let mut output = step(&mut app, &mut state, &[], 0.0);
    output = step(&mut app, &mut state, &click_on(&output, "Tags"), 1.0);
    let chip = output
        .zones
        .iter()
        .find(|zone| zone.label.starts_with("nginx ("))
        .map(|zone| zone.label.clone())
        .unwrap();
    output = step(&mut app, &mut state, &click_on(&output, &chip), 2.0);
    output = step(
        &mut app,
        &mut state,
        &click_on(&output, "Delete tag\u{2026}"),
        3.0,
    );
    // Cancel keeps it.
    output = step(&mut app, &mut state, &click_on(&output, "Cancel"), 4.0);
    assert_eq!(tagged(&app), before);
    output = step(
        &mut app,
        &mut state,
        &click_on(&output, "Delete tag\u{2026}"),
        5.0,
    );
    let yes = output
        .zones
        .iter()
        .find(|zone| zone.label.starts_with("Delete \u{201c}nginx\u{201d} from"))
        .map(|zone| zone.label.clone())
        .unwrap();
    step(&mut app, &mut state, &click_on(&output, &yes), 6.0);
    assert_eq!(tagged(&app), 0, "gone from every host");
}

#[test]
fn a_locked_vault_hides_every_open_tab_and_the_palette() {
    let mut app = NativeApp::new(Some(fixture_vault("locked-tabs", true)));
    replay(&mut app, "web-1 session");
    let mut state = UiState::default();
    step(&mut app, &mut state, &[], 0.0);
    app.lock_vault();
    let mut output = step(&mut app, &mut state, &[], 1.0);
    let names: Vec<&str> = output
        .zones
        .iter()
        .map(|zone| zone.label.as_str())
        .collect();
    assert!(!names.contains(&"web-1 session"), "{names:?}");
    assert!(
        !names.contains(&"New tab") && !names.contains(&"SFTP"),
        "no new tab while locked"
    );
    for keys in [
        vec![chord(shortcuts::NEXT_TAB)],
        vec![chord(shortcuts::PALETTE)],
        vec![chord(shortcuts::NEW_TAB)],
    ] {
        output = step(&mut app, &mut state, &keys, 2.0);
        assert_eq!(app.active, 0, "the lock screen stays in front");
        assert!(app.palette.is_none() && app.opens.is_empty());
    }
    assert!(
        output
            .zones
            .iter()
            .any(|zone| zone.label.contains("password"))
    );
}

#[test]
fn locking_drops_the_secrets_open_tabs_hold_and_unlocking_brings_them_back() {
    let mut vault = fixture_vault("locked-secrets", false);
    let mut host = vault.draft_host();
    host.label = "db".into();
    host.address = "10.0.4.12".into();
    host.username = "deploy".into();
    host.password = Some(annulus_vault::hosts::Secret::new("hunter2"));
    let id = host.id;
    vault.upsert_host(host).unwrap();
    let mut app = NativeApp::new(Some(vault));
    app.act(crate::native::hosts::HostsAction::Connect(id));
    let mut state = UiState::default();
    // One frame adds the tab; it is not drawn yet, so it does not connect.
    drive(&mut app, &mut state, &[], 0.0);
    let password = |app: &NativeApp| {
        app.tabs.iter().find_map(|tab| match &tab.terminal {
            Some(TermView::Live(panes)) => panes.programs().into_iter().next(),
            _ => None,
        })
    };
    let has_password = |program: Option<annulus_session::Program>| {
        matches!(
            program,
            Some(annulus_session::Program::Ssh {
                password: Some(_),
                ..
            })
        )
    };
    assert!(has_password(password(&app)), "the tab can log in");
    let path = app.vault.as_ref().unwrap().path().to_path_buf();
    app.lock_vault();
    assert!(
        !has_password(password(&app)),
        "locked: the password is gone from the tab"
    );
    // Unlocking brings the login back from the vault (the lock screen's job, done by hand).
    app.vault = Some(annulus_vault::vault::Vault::unlock(path, "pw").unwrap());
    app.unlocked();
    assert!(
        has_password(password(&app)),
        "unlocked: it can log in again"
    );
}

#[test]
fn the_palette_cannot_lock_while_the_recovery_key_is_shown() {
    let mut app = NativeApp::new(None);
    app.lock = crate::native::lock::LockView::new(crate::native::lock::Mode::Setup);
    let mut state = UiState::default();
    step(&mut app, &mut state, &[], 0.0);
    step(&mut app, &mut state, &[chord(shortcuts::PALETTE)], 1.0);
    assert!(app.palette.is_none(), "no palette before a vault is open");
}

#[test]
fn a_second_window_cannot_open_the_same_data_folder() {
    let dir = std::env::temp_dir().join(format!("annulus-instance-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let first = crate::native::instance_lock(&dir);
    assert!(first.is_some(), "the first window takes the folder");
    assert!(
        crate::native::instance_lock(&dir).is_none(),
        "a second one is refused"
    );
    drop(first);
    // Other tests start shells meanwhile: a child forked while the lock was open shares it
    // until it starts its program (the file closes on exec), so allow it a moment.
    let started = std::time::Instant::now();
    while crate::native::instance_lock(&dir).is_none() {
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "free again once the first closes"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

#[test]
fn a_key_pasted_into_the_keychain_is_saved_under_its_name() {
    let mut app = NativeApp::new(Some(fixture_vault("import-key", true)));
    app.section = crate::native::Section::Keychain;
    let pair = annulus_ssh::keys::generate(annulus_ssh::keys::KeyKind::Ed25519, "me@test").unwrap();
    let before = app.vault.as_ref().unwrap().ssh_keys().len();
    let mut state = UiState::default();
    let mut output = drive(&mut app, &mut state, &[], 0.0);
    output = step(&mut app, &mut state, &click_on(&output, "Import key"), 1.0);
    let mut events = click_on(&output, "Name, e.g. id_ed25519");
    events.push(Event::Text("deploy".into()));
    output = drive(&mut app, &mut state, &events, 2.0);
    let mut events = click_on(
        &output,
        "Paste the private key (-----BEGIN \u{2026} PRIVATE KEY-----)",
    );
    events.push(Event::Paste(pair.private.to_string()));
    output = drive(&mut app, &mut state, &events, 3.0);
    drive(&mut app, &mut state, &click_on(&output, "Import"), 4.0);
    let keys = app.vault.as_ref().unwrap().ssh_keys();
    assert_eq!(keys.len(), before + 1);
    let saved = keys.iter().find(|key| key.label == "deploy").unwrap();
    assert_eq!(
        (saved.label.as_str(), saved.public.as_str()),
        ("deploy", pair.public.as_str())
    );
}

/// Walks the forwarding wizard the way a user does: a kind, the ports, Next, then Save or
/// Save and start. Returns the frame after the last click.
fn through_the_wizard(
    app: &mut NativeApp,
    state: &mut UiState,
    clock: &dyn Fn() -> f64,
    (listen, host, port): (&str, &str, &str),
    last: &str,
) -> FrameOutput {
    let mut output = step(app, state, &[], clock());
    output = step(
        app,
        state,
        &click_on(&output, "Reach a service behind a server"),
        clock(),
    );
    for (field, text) in [
        ("Port on this computer", listen),
        ("Destination, as the server sees it: host", host),
        ("Destination, as the server sees it: port", port),
    ] {
        let mut events = click_on(&output, field);
        events.push(Event::Text(text.to_owned()));
        output = step(app, state, &events, clock());
    }
    output = step(app, state, &click_on(&output, "Next"), clock());
    step(app, state, &click_on(&output, last), clock())
}

#[test]
fn the_forwarding_wizard_saves_the_rule_it_describes() {
    let mut app = NativeApp::new(Some(fixture_vault("wizard-rule", true)));
    forwards_empty(&mut app);
    let mut state = UiState::default();
    let time = std::cell::Cell::new(0.0);
    let clock = || {
        time.set(time.get() + 1.0);
        time.get()
    };
    through_the_wizard(
        &mut app,
        &mut state,
        &clock,
        ("15432", "db.internal", "5432"),
        "Save",
    );
    let vault = app.vault.as_ref().unwrap();
    let [rule] = vault.forwards() else {
        panic!("one rule, not {:?}", vault.forwards());
    };
    let server = &vault.hosts()[0].label;
    assert_eq!(
        (
            rule.kind,
            rule.bind_port,
            rule.destination_host.as_str(),
            rule.destination_port,
            rule.host_id,
        ),
        (
            annulus_vault::forwards::ForwardKind::Local,
            15432,
            "db.internal",
            5432,
            vault.hosts()[0].id,
        )
    );
    assert_eq!(rule.label, format!("db.internal:5432 via {server}"));
}

#[test]
fn the_wizards_suggested_name_follows_the_ports_until_one_is_typed() {
    let mut app = NativeApp::new(Some(fixture_vault("wizard-name", true)));
    forwards_empty(&mut app);
    let mut state = UiState::default();
    let time = std::cell::Cell::new(0.0);
    let clock = || {
        time.set(time.get() + 1.0);
        time.get()
    };
    let server = app.vault.as_ref().unwrap().hosts()[0].label.clone();
    let mut output = through_the_wizard(
        &mut app,
        &mut state,
        &clock,
        ("15432", "db.internal", "5432"),
        "Back",
    );
    // Back at the ports, the destination becomes 54321: the suggestion follows it.
    let mut events = click_on(&output, "Destination, as the server sees it: port");
    events.extend([
        Event::Key(Key::End, Modifiers::NONE),
        Event::Text("1".into()),
    ]);
    output = step(&mut app, &mut state, &events, clock());
    output = step(&mut app, &mut state, &click_on(&output, "Next"), clock());
    // Typed in, the name stays through another Back and Next.
    let mut events = click_on(&output, "Name");
    events.extend([
        Event::Key(Key::End, Modifiers::NONE),
        Event::Text(" x".into()),
    ]);
    output = step(&mut app, &mut state, &events, clock());
    output = step(&mut app, &mut state, &click_on(&output, "Back"), clock());
    output = step(&mut app, &mut state, &click_on(&output, "Next"), clock());
    step(&mut app, &mut state, &click_on(&output, "Save"), clock());
    let vault = app.vault.as_ref().unwrap();
    let [rule] = vault.forwards() else {
        panic!("one rule, not {:?}", vault.forwards());
    };
    assert_eq!(rule.label, format!("db.internal:54321 via {server} x"));
}

#[test]
fn the_connecting_card_sits_near_the_middle_and_its_log_opens_downward() {
    let mut app = NativeApp::new(Some(fixture_vault("connect-card", false)));
    connecting(&mut app);
    let mut state = UiState::default();
    step(&mut app, &mut state, &[], 0.0);
    let output = step(&mut app, &mut state, &[], 5.0);
    let toggle = |output: &FrameOutput, label: &str| {
        output
            .zones
            .iter()
            .find(|zone| zone.label == label)
            .map(|zone| zone.rect)
            .unwrap()
    };
    // This demo tab starts with its log open.
    let open = toggle(&output, "Hide log");
    // The window is 800 tall: the card's head sits in the band a little above the middle.
    assert!((240.0..400.0).contains(&open.y), "{open:?}");
    step(&mut app, &mut state, &click_on(&output, "Hide log"), 6.0);
    let output = step(&mut app, &mut state, &[], 11.0);
    let closed = toggle(&output, "Show log");
    assert!(
        (open.y - closed.y).abs() < 1.0,
        "the log folds up under a card that stays: {closed:?}"
    );
}

#[test]
fn a_rule_open_to_the_network_says_so_on_its_row() {
    let mut app = NativeApp::new(Some(fixture_vault("open-rule", true)));
    forwards_open(&mut app);
    let mut state = UiState::default();
    let output = step(&mut app, &mut state, &[], 0.0);
    let open: Vec<&str> = output
        .zones
        .iter()
        .filter(|zone| zone.label.ends_with(", open to your network"))
        .map(|zone| zone.label.as_str())
        .collect();
    assert_eq!(open.len(), 1, "only the rule on 0.0.0.0: {open:?}");
}

#[test]
fn the_tags_button_tells_a_screen_reader_whether_its_chips_are_open() {
    let mut app = NativeApp::new(Some(fixture_vault("tags-toggle", true)));
    let mut state = UiState::default();
    let toggled = |output: &FrameOutput| {
        output
            .zones
            .iter()
            .find(|zone| zone.label == "Tags")
            .and_then(|zone| zone.toggled)
    };
    let output = step(&mut app, &mut state, &[], 0.0);
    assert_eq!(toggled(&output), Some(false));
    let output = step(&mut app, &mut state, &click_on(&output, "Tags"), 1.0);
    assert_eq!(toggled(&output), Some(true));
}

#[test]
fn the_wizards_cards_are_reached_with_tab_and_picked_with_enter() {
    let mut app = NativeApp::new(Some(fixture_vault("wizard-keys", true)));
    forwards_empty(&mut app);
    let mut state = UiState::default();
    let output = step(&mut app, &mut state, &[], 0.0);
    let card = output
        .zones
        .iter()
        .find(|zone| zone.label == "Share a service from this computer")
        .map(|zone| zone.id)
        .unwrap();
    let tab = [Event::Key(Key::Tab, Modifiers::NONE)];
    let mut time = 1.0;
    while state.focus() != Some(card) {
        assert!(time < 60.0, "Tab never reached the card");
        step(&mut app, &mut state, &tab, time);
        time += 1.0;
    }
    let enter = [Event::Key(Key::Enter, Modifiers::NONE)];
    let output = step(&mut app, &mut state, &enter, time);
    assert!(
        output
            .zones
            .iter()
            .any(|zone| zone.label == "Port on the server"),
        "Enter picked it: the remote rule's ports"
    );
}

#[test]
fn duplicating_a_rule_opens_a_new_one_that_saves_beside_it() {
    let mut app = NativeApp::new(Some(fixture_vault("duplicate-rule", true)));
    forwards(&mut app);
    let before = app.vault.as_ref().unwrap().forwards().len();
    let mut state = UiState::default();
    let mut output = step(&mut app, &mut state, &[], 0.0);
    output = step(
        &mut app,
        &mut state,
        &click_on(&output, "Duplicate rule"),
        1.0,
    );
    let mut events = click_on(&output, "Port, e.g. 8080");
    events.push(Event::Text("18080".into()));
    output = step(&mut app, &mut state, &events, 2.0);
    step(&mut app, &mut state, &click_on(&output, "Save"), 3.0);
    let rules = app.vault.as_ref().unwrap().forwards();
    assert_eq!(rules.len(), before + 1, "the copy is saved beside it");
    let copy = rules.iter().find(|rule| rule.bind_port == 18080).unwrap();
    assert!(copy.label.ends_with("copy"), "{}", copy.label);
}

/// End to end on test VM 1: a local rule made in the forwarding wizard and saved with Save
/// and start carries a connection to the VM's own sshd.
#[test]
#[ignore = "needs the test servers"]
fn e2e_a_rule_made_in_the_wizard_carries_traffic_through_a_real_host() {
    use std::io::Read as _;
    let var = |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("{name} unset"));
    let mut vault = fixture_vault("e2e-wizard", false);
    let login = annulus_ssh::keys::import(
        &std::fs::read_to_string(var("ANNULUS_E2E_KEY")).unwrap(),
        None,
    )
    .unwrap();
    let login = crate::keychain_screen::stored(&vault, "e2e", &login);
    let key_id = login.id;
    vault.upsert_key(login).unwrap();
    let mut host = vault.draft_host();
    host.label = "test-1".into();
    host.address = var("ANNULUS_E2E_VM1");
    host.username = var("ANNULUS_E2E_USER");
    host.key_id = Some(key_id);
    vault.upsert_host(host).unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut app = NativeApp::new(Some(vault));
    app.section = crate::native::Section::PortForwarding;
    let dir = std::env::temp_dir().join(format!("annulus-e2e-wizard-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    app.ssh_files.known_hosts = dir.join("known_hosts");
    app.ssh_files.key_dir = None;
    let mut state = UiState::default();
    let started = std::time::Instant::now();
    let clock = || started.elapsed().as_secs_f64();
    let mut output = through_the_wizard(
        &mut app,
        &mut state,
        &clock,
        (&port.to_string(), "127.0.0.1", "22"),
        "Save and start",
    );
    let banner = loop {
        assert!(clock() < 60.0, "the forward never answered");
        let trust = output
            .zones
            .iter()
            .any(|zone| zone.label == "Trust and connect");
        let events = if trust {
            click_on(&output, "Trust and connect")
        } else {
            Vec::new()
        };
        output = step(&mut app, &mut state, &events, clock());
        if let Ok(mut stream) = std::net::TcpStream::connect(("127.0.0.1", port)) {
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut bytes = [0_u8; 8];
            if stream.read_exact(&mut bytes).is_ok() {
                break String::from_utf8_lossy(&bytes).into_owned();
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    };
    assert!(banner.starts_with("SSH-2.0"), "{banner:?}");
}

/// End to end on test VM 1: the new Keychain installs a fresh key on a real host through
/// the UI (Install on a host, choose it, trust its key, Install), and the host is set to log
/// in with it. Deleting the key then logs in with it to take it off the host, and the host's
/// `authorized_keys` no longer holds it.
#[test]
#[ignore = "needs the test servers"]
#[allow(clippy::too_many_lines)] // Install, delete, check the server.
fn e2e_the_new_keychain_installs_a_key_and_deleting_it_takes_it_off_the_host() {
    use crate::keychain_screen::stored;
    use annulus_ssh::keys;

    let var = |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("{name} unset"));
    let mut vault = fixture_vault("e2e-install", false);
    // The e2e key logs in the first time; the fresh key is the one installed.
    let login = keys::import(
        &std::fs::read_to_string(var("ANNULUS_E2E_KEY")).unwrap(),
        None,
    )
    .unwrap();
    let login = stored(&vault, "e2e", &login);
    let login_id = login.id;
    vault.upsert_key(login).unwrap();
    let marker = format!("annulus-ui-e2e-{}", std::process::id());
    let fresh = keys::generate(keys::KeyKind::Ed25519, &marker).unwrap();
    let fresh = stored(&vault, "fresh", &fresh);
    let fresh_id = fresh.id;
    vault.upsert_key(fresh).unwrap();
    let mut host = vault.draft_host();
    host.label = "test-1".into();
    host.address = var("ANNULUS_E2E_VM1");
    host.username = var("ANNULUS_E2E_USER");
    host.key_id = Some(login_id);
    let host_id = host.id;
    vault.upsert_host(host).unwrap();
    let mut app = NativeApp::new(Some(vault));
    app.section = crate::native::Section::Keychain;
    let dir = std::env::temp_dir().join(format!("annulus-e2e-install-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    app.ssh_files.known_hosts = dir.join("known_hosts");
    let mut state = UiState::default();
    let started = std::time::Instant::now();
    let clock = || started.elapsed().as_secs_f64();
    // `step` draws ahead of the time it is given, so frames keep their own clock: a click on a
    // card that has arrived must not land in a frame from before it did.
    let drawn = std::cell::Cell::new(0.0);
    let frame = || {
        drawn.set(drawn.get() + 1.0);
        drawn.get()
    };
    let mut output = step(&mut app, &mut state, &[], frame());
    let install = click_within(&output, "fresh", "Install on a host");
    output = step(&mut app, &mut state, &install, frame());
    output = step(
        &mut app,
        &mut state,
        &click_on(&output, "Choose a host"),
        frame(),
    );
    output = step(&mut app, &mut state, &click_on(&output, "test-1"), frame());
    output = step(&mut app, &mut state, &click_on(&output, "Install"), frame());
    let key_of = |app: &NativeApp| {
        app.vault
            .as_ref()
            .and_then(|vault| vault.hosts().iter().find(|host| host.id == host_id))
            .and_then(|host| host.key_id)
    };
    while key_of(&app) != Some(fresh_id) {
        assert!(clock() < 60.0, "the key was not installed");
        let trust = output
            .zones
            .iter()
            .any(|zone| zone.label == "Trust and connect");
        let events = if trust {
            click_on(&output, "Trust and connect")
        } else {
            Vec::new()
        };
        output = step(&mut app, &mut state, &events, frame());
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    // Delete it from the Keychain: it logs in with the fresh key to take it off the host,
    // then leaves the vault.
    output = step(&mut app, &mut state, &[], frame());
    let delete = click_within(&output, "fresh", "Delete");
    output = step(&mut app, &mut state, &delete, frame());
    step(
        &mut app,
        &mut state,
        &click_on(&output, "Delete key"),
        frame(),
    );
    let has_fresh = |app: &NativeApp| {
        app.vault
            .as_ref()
            .is_some_and(|vault| vault.ssh_keys().iter().any(|key| key.id == fresh_id))
    };
    while has_fresh(&app) {
        assert!(clock() < 90.0, "the key was not taken off and deleted");
        step(&mut app, &mut state, &[], frame());
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    // Logged in with the e2e key again, the host no longer lists it.
    let vault = app.vault.as_mut().unwrap();
    let mut host = vault
        .hosts()
        .iter()
        .find(|host| host.id == host_id)
        .cloned()
        .unwrap();
    assert_eq!(
        host.key_id, None,
        "the host no longer names the deleted key"
    );
    host.key_id = Some(login_id);
    vault.upsert_host(host.clone()).unwrap();
    let task = crate::ssh_task::SshTask::exec(
        crate::connect::host(vault, &app.ssh_files, &host),
        on_linux(format!("grep -c '{marker}' ~/.ssh/authorized_keys || true")),
        || {},
    );
    let outcome = loop {
        assert!(clock() < 120.0, "the check never finished");
        if let Some(outcome) = task.outcome() {
            break outcome;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    let exec = outcome.unwrap();
    assert_eq!(String::from_utf8_lossy(&exec.output).trim(), "0");
}

/// `command` for the test VMs, which run Linux.
fn on_linux(command: String) -> annulus_ssh::remote::Commands {
    annulus_ssh::remote::Commands {
        windows: command.clone(),
        unix: command,
    }
}

/// How many SSH sessions `host`'s user has on the server, counted over one more connection
/// (the count includes it, the same way every time). Host-key questions are trusted.
fn sessions_on(app: &NativeApp, host_id: u64) -> usize {
    let vault = app.vault.as_ref().unwrap();
    let host = vault
        .hosts()
        .iter()
        .find(|host| host.id == host_id)
        .unwrap();
    let user = host.username.clone();
    let mut task = crate::ssh_task::SshTask::exec(
        crate::connect::host(vault, &app.ssh_files, host),
        on_linux(format!("pgrep -fc '^sshd: {user}' || true")),
        || {},
    );
    let started = std::time::Instant::now();
    loop {
        assert!(
            started.elapsed().as_secs() < 30,
            "the count never came back"
        );
        while let Some(event) = task.question() {
            if let annulus_session::SessionEvent::HostKey(_) = event {
                task.answers().host_key(true);
            }
        }
        if let Some(outcome) = task.outcome() {
            let exec = outcome.unwrap();
            return String::from_utf8_lossy(&exec.output)
                .trim()
                .parse()
                .unwrap();
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// End to end on test VM 1: a terminal tab, an SFTP tab and a port forward opened through the
/// app, then closed the way a user closes them (the tab's close, the rule's switch), leave
/// nothing behind: the server is back to the sessions it had, and the forwarded port is free.
#[test]
#[ignore = "needs the test servers"]
#[allow(clippy::too_many_lines)] // Open four connections, check them, close them, check again.
fn e2e_closing_terminals_sftp_and_forwards_leaves_nothing_on_the_server() {
    use std::io::Read as _;
    let var = |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("{name} unset"));
    let mut vault = fixture_vault("e2e-cleanup", false);
    let login = annulus_ssh::keys::import(
        &std::fs::read_to_string(var("ANNULUS_E2E_KEY")).unwrap(),
        None,
    )
    .unwrap();
    let login = crate::keychain_screen::stored(&vault, "e2e", &login);
    let key_id = login.id;
    vault.upsert_key(login).unwrap();
    let mut host = vault.draft_host();
    host.label = "test-1".into();
    host.address = var("ANNULUS_E2E_VM1");
    host.username = var("ANNULUS_E2E_USER");
    host.key_id = Some(key_id);
    let host_id = host.id;
    vault.upsert_host(host).unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut rule = vault.draft_forward(host_id);
    rule.label = "ssh through test-1".into();
    rule.kind = annulus_vault::forwards::ForwardKind::Local;
    rule.bind_port = port;
    rule.destination_host = "127.0.0.1".into();
    rule.destination_port = 22;
    vault.upsert_forward(rule).unwrap();
    let mut app = NativeApp::new(Some(vault));
    let dir = std::env::temp_dir().join(format!("annulus-e2e-cleanup-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    app.ssh_files.known_hosts = dir.join("known_hosts");
    app.ssh_files.key_dir = None;
    let before = sessions_on(&app, host_id);
    let mut state = UiState::default();
    let started = std::time::Instant::now();
    let clock = || started.elapsed().as_secs_f64();
    // A terminal, split into two panes, and an SFTP tab.
    app.act(crate::native::hosts::HostsAction::Connect(host_id));
    for _ in 0..3 {
        drive(&mut app, &mut state, &[], clock());
    }
    let split = [chord(shortcuts::SPLIT_RIGHT)];
    drive(&mut app, &mut state, &split, clock());
    app.act(crate::native::hosts::HostsAction::Sftp(host_id));
    for _ in 0..3 {
        drive(&mut app, &mut state, &[], clock());
    }
    // The forward, switched on from its row, carries a connection to the server's sshd.
    app.active = 0;
    app.section = crate::native::Section::PortForwarding;
    let mut output = step(&mut app, &mut state, &[], clock());
    step(
        &mut app,
        &mut state,
        &click_within(&output, "ssh through test-1", "Run this rule"),
        clock(),
    );
    let banner = loop {
        assert!(clock() < 60.0, "the forward never answered");
        drive(&mut app, &mut state, &[], clock());
        if let Ok(mut stream) = std::net::TcpStream::connect(("127.0.0.1", port)) {
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut bytes = [0_u8; 8];
            if stream.read_exact(&mut bytes).is_ok() {
                break String::from_utf8_lossy(&bytes).into_owned();
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    };
    assert!(banner.starts_with("SSH-2.0"), "{banner:?}");
    // Every one of them shows on the server: two panes, the files tab and the forward.
    let during = loop {
        drive(&mut app, &mut state, &[], clock());
        let count = sessions_on(&app, host_id);
        if count >= before + 4 || clock() > 60.0 {
            break count;
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    };
    assert!(
        during >= before + 4,
        "{during} sessions while open, {before} before"
    );
    // Closed as a user closes them.
    output = step(&mut app, &mut state, &[], clock());
    step(
        &mut app,
        &mut state,
        &click_within(&output, "ssh through test-1", "Run this rule"),
        clock(),
    );
    // The fixed tabs (Vault, SFTP) never close.
    while app.tabs.len() > crate::native::FIXED_TABS {
        assert!(clock() < 90.0, "{} tabs left open", app.tabs.len());
        app.close_tab(app.tabs.len() - 1);
        drive(&mut app, &mut state, &[], clock());
    }
    loop {
        drive(&mut app, &mut state, &[], clock());
        let count = sessions_on(&app, host_id);
        let free = std::net::TcpListener::bind(("127.0.0.1", port)).is_ok();
        if count == before && free {
            break;
        }
        assert!(
            clock() < 120.0,
            "after closing: {count} sessions on the server (before: {before}), port free: {free}"
        );
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

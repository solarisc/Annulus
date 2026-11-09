#[test]
fn an_agent_runs_a_command_through_the_door_in_its_own_tab_and_gets_its_output() {
    use annulus_control::client::Client;
    let _timed = timing_a_shell();
    let mut app = NativeApp::new(Some(fixture_vault("agent-run", false)));
    app.vault_path = app.vault.as_ref().map(|vault| vault.path().to_path_buf());
    let dir = app
        .vault_path
        .as_ref()
        .and_then(|path| path.parent())
        .map(std::path::Path::to_path_buf)
        .unwrap();
    app.waker = Some(std::sync::Arc::new(|| {}));
    app.settings.automation = true;
    let mut state = UiState::default();
    step(&mut app, &mut state, &[], 0.0);
    let agent = std::thread::spawn(move || {
        let mut client = Client::connect(&dir).map_err(|e| e.to_string())?;
        let tab = client.open(None).map_err(|e| e.to_string())?;
        let ran = client
            .run(tab.id, "echo agent-was-here", Some(30))
            .map_err(|e| e.to_string())?;
        Ok::<_, String>((tab.id, ran))
    });
    let started = std::time::Instant::now();
    let mut time = 1.0;
    while !agent.is_finished() && started.elapsed() < std::time::Duration::from_secs(90) {
        step(&mut app, &mut state, &[], time);
        time += 0.05;
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    let (tab, ran) = agent.join().unwrap().unwrap();
    assert!(
        ran.output
            .lines()
            .any(|line| line.trim() == "agent-was-here"),
        "the echo's answer comes back: {ran:?}"
    );
    assert_eq!(
        ran.state,
        annulus_control::protocol::RunState::Done,
        "its prompt came back: {ran:?}"
    );
    let log = app
        .tabs
        .iter()
        .find(|open| open.uid == tab)
        .and_then(|open| open.agent.as_ref())
        .unwrap();
    assert_eq!(log.commands.len(), 1);
    let command = &log.commands[0];
    assert_eq!(command.text, "echo agent-was-here", "listed beside the tab");
    assert!(
        matches!(command.state, crate::native::control::Ran::Done { lines: 1.., .. }),
        "done, with the lines it printed"
    );
    let line = command.line;
    assert_eq!(log.marks().lines, vec![line], "marked in the margin");

    // Picking it in the panel shows it; picking it again lets it go.
    let output = step(&mut app, &mut state, &[], time);
    let output = step(
        &mut app,
        &mut state,
        &click_on(&output, "echo agent-was-here"),
        time + 0.1,
    );
    let log = app
        .tabs
        .iter()
        .find(|open| open.uid == tab)
        .and_then(|open| open.agent.as_ref());
    assert_eq!(log.and_then(|log| log.picked), Some(0));
    assert_eq!(
        log.and_then(|log| log.marks().picked)
            .map(|lines| lines.start),
        Some(line),
        "its lines are tinted from the one it was typed on"
    );
    step(
        &mut app,
        &mut state,
        &click_on(&output, "echo agent-was-here"),
        time + 0.2,
    );
    let log = app
        .tabs
        .iter()
        .find(|open| open.uid == tab)
        .and_then(|open| open.agent.as_ref());
    assert_eq!(log.and_then(|log| log.picked), None);
}

#[test]
fn an_agent_hears_why_its_tab_could_not_connect_and_its_command_is_refused_at_once() {
    use annulus_control::client::Client;
    let _timed = timing_a_shell();
    let mut app = NativeApp::new(Some(fixture_vault("agent-failed", false)));
    app.vault_path = app.vault.as_ref().map(|vault| vault.path().to_path_buf());
    let dir = app
        .vault_path
        .as_ref()
        .and_then(|path| path.parent())
        .map(std::path::Path::to_path_buf)
        .unwrap();
    app.waker = Some(std::sync::Arc::new(|| {}));
    app.settings.automation = true;
    let broken = annulus_session::Program::Command {
        program: "annulus-no-such-program".into(),
        args: Vec::new(),
    };
    let tab = app.open_terminal("broken".into(), broken, None);
    if let Some(open) = app.tabs.iter_mut().find(|open| open.uid == tab) {
        open.agent = Some(Box::new(crate::native::control::AgentLog::new(0)));
    }
    let mut state = UiState::default();
    // The first frame opens the door and tries to start the program.
    step(&mut app, &mut state, &[], 0.0);
    let agent = std::thread::spawn(move || {
        let mut client = Client::connect(&dir).map_err(|e| e.to_string())?;
        let read = client.read(tab, None).map_err(|e| e.to_string())?;
        let run = client.run(tab, "echo never", Some(30));
        Ok::<_, String>((read, run.map_err(|e| e.to_string())))
    });
    let started = std::time::Instant::now();
    let mut time = 0.05;
    while !agent.is_finished() && started.elapsed() < std::time::Duration::from_secs(20) {
        step(&mut app, &mut state, &[], time);
        time += 0.05;
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let (read, run) = agent.join().unwrap().unwrap();
    assert!(read.contains("[Annulus] Could not start"), "{read:?}");
    let refused = run.unwrap_err();
    assert!(
        refused.contains("could not connect") && refused.contains("Could not start"),
        "{refused}"
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "refused at once, not after the 30 s start wait"
    );
}

/// Waits for agent tab `tab` to show its shell's prompt. A shell silent for 10 s has a command
/// typed blind, and on a runner busy with another job a new PowerShell can be that slow.
fn await_prompt(client: &mut annulus_control::client::Client, tab: u64) -> Result<(), String> {
    let started = std::time::Instant::now();
    loop {
        let shown = client.read(tab, None).map_err(|e| e.to_string())?;
        if shown.trim_end().ends_with(['>', '$', '#', '%']) {
            return Ok(());
        }
        if started.elapsed() > std::time::Duration::from_secs(60) {
            return Err(format!("no prompt within a minute: {shown:?}"));
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

#[test]
fn an_agents_command_runs_out_of_its_wait_is_interrupted_and_stop_cuts_off_the_next() {
    use crate::native::control::Ran;
    use annulus_control::client::Client;
    let _timed = timing_a_shell();
    // A line a second for a while, in every local shell the runners have.
    let ticking = if cfg!(windows) {
        "ping -n 40 127.0.0.1"
    } else {
        "for i in $(seq 40); do echo tick $i; sleep 1; done"
    };
    let mut app = NativeApp::new(Some(fixture_vault("agent-stop", false)));
    app.vault_path = app.vault.as_ref().map(|vault| vault.path().to_path_buf());
    let dir = app
        .vault_path
        .as_ref()
        .and_then(|path| path.parent())
        .map(std::path::Path::to_path_buf)
        .unwrap();
    app.waker = Some(std::sync::Arc::new(|| {}));
    app.settings.automation = true;
    let mut state = UiState::default();
    // The first frame opens the door.
    step(&mut app, &mut state, &[], 0.0);
    let mut time = 0.05;
    let mut frames = |app: &mut NativeApp, until: &dyn Fn(&NativeApp) -> bool| {
        let started = std::time::Instant::now();
        while !until(app) && started.elapsed() < std::time::Duration::from_secs(90) {
            step(app, &mut state, &[], time);
            time += 0.05;
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
    };
    let states = |app: &NativeApp| -> Vec<String> {
        app.tabs
            .iter()
            .filter_map(|tab| tab.agent.as_ref())
            .flat_map(|log| &log.commands)
            .map(|command| match command.state {
                Ran::Running(_) => "running",
                Ran::Done { .. } => "done",
                Ran::Waiting { .. } => "waiting",
                Ran::TimedOut { .. } => "timed out",
                Ran::Interrupted { .. } => "interrupted",
                Ran::Cut { .. } => "cut",
                Ran::Ended { .. } => "ended",
            })
            .map(str::to_owned)
            .collect()
    };

    let agent = std::thread::spawn(move || {
        let mut client = Client::connect(&dir).map_err(|e| e.to_string())?;
        let tab = client.open(None).map_err(|e| e.to_string())?;
        await_prompt(&mut client, tab.id)?;
        let out = client
            // A wait shorter than the quiet that counts as done: it always runs out, however
            // slowly a busy runner draws frames.
            .run(tab.id, ticking, Some(1))
            .map_err(|e| e.to_string())?;
        client.interrupt(tab.id).map_err(|e| e.to_string())?;
        // Answered only once this one settles, which Stop never lets happen.
        let cut = client.run(tab.id, ticking, Some(60));
        Ok::<_, String>((out, cut.err().map(|e| e.to_string())))
    });
    frames(&mut app, &|app| {
        states(app).len() == 2 || agent.is_finished()
    });
    assert!(!agent.is_finished(), "{:?}", agent.join());
    assert_eq!(states(&app), ["interrupted", "running"]);

    // Stop, as the panel's button does.
    app.settings.automation = false;
    frames(&mut app, &|app| {
        states(app).last().is_some_and(|last| last == "cut")
    });
    assert_eq!(
        states(&app),
        ["interrupted", "cut"],
        "nothing is left spinning"
    );
    let (out, refused) = agent.join().unwrap().unwrap();
    // Its first line may or may not come within the second a busy runner gives it, nor even
    // the shell's echo of what was typed: what counts is that the answer starts at the
    // prompt the command was typed at and says it still runs.
    let typed_at = out.output.lines().next().unwrap_or_default().trim_end();
    assert!(
        typed_at.contains(ticking) || typed_at.ends_with(['>', '$', '#', '%']),
        "{out:?}"
    );
    assert_eq!(
        out.state,
        annulus_control::protocol::RunState::Running,
        "out of its wait, it is still running: {out:?}"
    );
    assert!(refused.is_some(), "the cut-off agent hears no answer");
}

#[test]
fn an_agents_silent_command_is_not_done_until_its_prompt_is_back_and_wait_follows_it() {
    use annulus_control::client::Client;
    use annulus_control::protocol::{RunOutput, RunState};
    let _timed = timing_a_shell();
    let mut app = NativeApp::new(Some(fixture_vault("agent-silent", false)));
    app.vault_path = app.vault.as_ref().map(|vault| vault.path().to_path_buf());
    let dir = app
        .vault_path
        .as_ref()
        .and_then(|path| path.parent())
        .map(std::path::Path::to_path_buf)
        .unwrap();
    app.waker = Some(std::sync::Arc::new(|| {}));
    app.settings.automation = true;
    // Three seconds of silence, then one line, in the shell an agent's tab opens.
    let shell = app
        .profiles
        .first()
        .map(|profile| profile.name.clone())
        .unwrap_or_default();
    let command = if !cfg!(windows) {
        "sleep 3; echo slept-then-said"
    } else if shell.contains("PowerShell") {
        "Start-Sleep -Seconds 3; Write-Output slept-then-said"
    } else {
        "ping -n 4 127.0.0.1 >nul & echo slept-then-said"
    };
    let mut state = UiState::default();
    // The first frame opens the door.
    step(&mut app, &mut state, &[], 0.0);
    let agent = std::thread::spawn(move || {
        let mut client = Client::connect(&dir).map_err(|e| e.to_string())?;
        let tab = client.open(None).map_err(|e| e.to_string())?;
        // Its wait runs out in the silence: answered, but not as done.
        let early = client
            .run(tab.id, command, Some(1))
            .map_err(|e| e.to_string())?;
        let late = client.wait(tab.id, Some(30)).map_err(|e| e.to_string())?;
        Ok::<_, String>((tab.id, early, late))
    });
    let started = std::time::Instant::now();
    let mut time = 0.05;
    while !agent.is_finished() && started.elapsed() < std::time::Duration::from_secs(90) {
        step(&mut app, &mut state, &[], time);
        time += 0.05;
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    let (tab, early, late) = agent.join().unwrap().unwrap();
    // The typed command holds the word too: only a line of its own is the answer.
    let said = |ran: &RunOutput| {
        ran.output
            .lines()
            .any(|line| line.trim() == "slept-then-said")
    };
    assert_eq!(early.state, RunState::Running, "{early:?}");
    assert!(!said(&early), "{early:?}");
    assert_eq!(late.state, RunState::Done, "{late:?}");
    assert!(said(&late), "the line after the silence: {late:?}");
    assert!(
        late.millis >= 2_000,
        "done once its prompt came back, after the silence: {late:?}"
    );
    let log = app
        .tabs
        .iter()
        .find(|open| open.uid == tab)
        .and_then(|open| open.agent.as_ref())
        .unwrap();
    assert!(
        matches!(log.commands[0].state, crate::native::control::Ran::Done { .. }),
        "its row turned done too"
    );
}

#[test]
fn an_agents_tab_opened_behind_starts_unseen_and_closes_once_its_shell_ends() {
    use annulus_control::client::Client;
    use annulus_control::protocol::RunState;
    let _timed = timing_a_shell();
    let mut app = NativeApp::new(Some(fixture_vault("agent-behind", false)));
    app.vault_path = app.vault.as_ref().map(|vault| vault.path().to_path_buf());
    let dir = app
        .vault_path
        .as_ref()
        .and_then(|path| path.parent())
        .map(std::path::Path::to_path_buf)
        .unwrap();
    app.waker = Some(std::sync::Arc::new(|| {}));
    app.settings.automation = true;
    app.settings.agent_tabs_in_front = false;
    app.settings.close_ended_agent_tabs = true;
    let mut state = UiState::default();
    // The first frame opens the door.
    step(&mut app, &mut state, &[], 0.0);
    let in_use = app.tabs[app.active].uid;
    let agent = std::thread::spawn(move || {
        let mut client = Client::connect(&dir).map_err(|e| e.to_string())?;
        let tab = client.open(None).map_err(|e| e.to_string())?;
        let said = client
            .run(tab.id, "echo unseen-ok", Some(30))
            .map_err(|e| e.to_string())?;
        let left = client
            .run(tab.id, "exit", Some(30))
            .map_err(|e| e.to_string())?;
        Ok::<_, String>((tab.id, said, left))
    });
    let started = std::time::Instant::now();
    let mut time = 0.05;
    while !agent.is_finished() && started.elapsed() < std::time::Duration::from_secs(90) {
        step(&mut app, &mut state, &[], time);
        time += 0.05;
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    let (tab, said, left) = agent.join().unwrap().unwrap();
    assert_eq!(app.tabs[app.active].uid, in_use, "it never came in front");
    assert_eq!(said.state, RunState::Done, "its shell ran unseen: {said:?}");
    assert!(
        said.output.lines().any(|line| line.trim() == "unseen-ok"),
        "{said:?}"
    );
    assert_eq!(left.state, RunState::Ended, "{left:?}");
    step(&mut app, &mut state, &[], time);
    assert!(
        app.tabs.iter().any(|open| open.uid == tab),
        "kept while agents may not close tabs, though the agent typed exit"
    );
    app.settings.stop_agents_closing_tabs = false;
    step(&mut app, &mut state, &[], time + 0.05);
    assert!(
        !app.tabs.iter().any(|open| open.uid == tab),
        "closed once its shell ended"
    );
}

#[test]
fn the_folded_agent_rail_is_a_card_clear_of_the_window_edge_and_its_tally_opens_the_panel() {
    let mut app = NativeApp::new(Some(fixture_vault("agent-rail", false)));
    agent_tab_folded(&mut app);
    let mut state = UiState::default();
    let output = step(&mut app, &mut state, &[], 0.0);
    let stop = output
        .zones
        .iter()
        .find(|zone| zone.label == "Stop all agents")
        .map(|zone| zone.rect)
        .expect("Stop sits on the rail");
    // A card like the terminal's beside it, clear of the window's right and bottom edges
    // (the drawn window is 1100 by 800).
    assert!(stop.r() <= 1100.0 - crate::native::PANEL_GAP, "{stop:?}");
    assert!(stop.b() <= 800.0 - crate::native::PANEL_GAP, "{stop:?}");
    let tally = output
        .zones
        .iter()
        .find(|zone| zone.label.contains("5 commands"))
        .map(|zone| zone.label.clone())
        .expect("the rail says how many commands, and how many still run");
    assert!(tally.contains("2 running"), "{tally}");
    step(&mut app, &mut state, &click_on(&output, &tally), 1.0);
    assert!(
        app.tabs[crate::native::FIXED_TABS]
            .agent
            .as_ref()
            .is_some_and(|log| !log.folded),
        "a click on the tally opens the panel"
    );
}

#[test]
fn a_host_pinned_from_a_new_tab_heads_the_next_one_and_a_second_click_unpins_it() {
    let mut app = NativeApp::new(Some(fixture_vault("new-tab-pin", true)));
    app.open_new_tab();
    let mut state = UiState::default();
    let output = step(&mut app, &mut state, &[], 0.0);
    // The highlighted (first) row shows its pin.
    step(
        &mut app,
        &mut state,
        &click_on(&output, "Pin to the top of New Tab"),
        1.0,
    );
    assert_eq!(app.settings.pinned.len(), 1, "pinned");
    let pinned = app.settings.pinned[0];
    assert_eq!(
        app.tabs[app.active].kind,
        crate::native::TabKind::NewTab,
        "the page stays to pin more"
    );
    let output = step(&mut app, &mut state, &[], 2.0);
    let label = app
        .vault
        .as_ref()
        .unwrap()
        .hosts()
        .iter()
        .find(|host| host.id == pinned)
        .unwrap()
        .label
        .clone();
    let first = output
        .zones
        .iter()
        .find(|zone| zone.role == Role::ListItem)
        .map(|zone| zone.label.clone());
    assert_eq!(first, Some(label), "it heads the list");
    step(
        &mut app,
        &mut state,
        &click_on(&output, "Unpin from New Tab"),
        3.0,
    );
    assert!(app.settings.pinned.is_empty(), "and unpins");
}

#[test]
fn an_empty_vaults_new_tab_welcomes_and_offers_the_ssh_config_import() {
    let mut app = NativeApp::new(Some(fixture_vault("new-tab-welcome", false)));
    app.open_new_tab();
    let mut state = UiState::default();
    let output = step(&mut app, &mut state, &[], 0.0);
    assert!(
        output
            .zones
            .iter()
            .any(|zone| zone.label == "New host" && zone.role == Role::Button)
    );
    step(
        &mut app,
        &mut state,
        &click_on(&output, "Import ssh_config"),
        1.0,
    );
    assert_eq!(app.active, 0, "to the vault");
    assert_eq!(app.section, crate::native::Section::Hosts);
    assert!(
        !app.tabs
            .iter()
            .any(|tab| tab.kind == crate::native::TabKind::NewTab),
        "the new tab gave way"
    );
}

#[test]
fn a_tabs_menu_closes_the_tabs_to_its_right_or_all_the_others() {
    let mut app = NativeApp::new(Some(fixture_vault("tab-menu", false)));
    for title in ["a", "b", "c", "d"] {
        replay(&mut app, title);
    }
    let mut state = UiState::default();
    let output = step(&mut app, &mut state, &[], 0.0);
    let output = step(&mut app, &mut state, &right_click_on(&output, "b"), 1.0);
    step(
        &mut app,
        &mut state,
        &click_on(&output, "Close tabs to the right"),
        2.0,
    );
    assert_eq!(titles(&app), ["Vault", "SFTP", "a", "b"]);
    assert_eq!(app.tabs[app.active].title, "b");
    replay(&mut app, "e");
    let output = step(&mut app, &mut state, &[], 3.0);
    let output = step(&mut app, &mut state, &right_click_on(&output, "b"), 4.0);
    step(
        &mut app,
        &mut state,
        &click_on(&output, "Close other tabs"),
        5.0,
    );
    assert_eq!(titles(&app), ["Vault", "SFTP", "b"]);
}

#[test]
fn a_tab_moves_among_the_sessions_and_the_one_in_front_stays_in_front() {
    let mut app = NativeApp::new(None);
    for title in ["a", "b", "c"] {
        replay(&mut app, title);
    }
    app.active = 2;
    app.move_tab(2, 4);
    assert_eq!(titles(&app), ["Vault", "SFTP", "b", "c", "a"]);
    assert_eq!(app.tabs[app.active].title, "a");
    // Fixed tabs never move, and nothing moves in among them.
    app.move_tab(0, 3);
    app.move_tab(4, 1);
    assert_eq!(titles(&app), ["Vault", "SFTP", "b", "c", "a"]);
    // By keyboard: the tab in front, one place left.
    let mut state = UiState::default();
    app.vault = Some(fixture_vault("move-tab", false));
    step(&mut app, &mut state, &[], 0.0);
    step(&mut app, &mut state, &[chord(shortcuts::MOVE_LEFT)], 1.0);
    assert_eq!(titles(&app), ["Vault", "SFTP", "b", "a", "c"]);
    assert_eq!(app.tabs[app.active].title, "a");
}

#[test]
fn a_tab_dragged_past_its_neighbour_takes_its_place() {
    let mut app = NativeApp::new(Some(fixture_vault("drag-tab", false)));
    for title in ["first", "second"] {
        replay(&mut app, title);
    }
    let mut state = UiState::default();
    let output = step(&mut app, &mut state, &[], 0.0);
    let from = zone_center(&output.zones, "first");
    let past = zone_center(&output.zones, "second");
    let drag = [
        Event::PointerMoved(from),
        Event::PointerDown(Button::Primary),
        Event::PointerMoved(Point::new(past.x + 40.0, past.y)),
    ];
    step(&mut app, &mut state, &drag, 1.0);
    step(
        &mut app,
        &mut state,
        &[Event::PointerUp(Button::Primary)],
        2.0,
    );
    assert_eq!(titles(&app), ["Vault", "SFTP", "second", "first"]);
    assert_eq!(
        app.tabs[app.active].title, "first",
        "the dragged tab is in front"
    );
}

#[test]
fn idle_time_at_or_past_the_timeout_locks() {
    let mut app = NativeApp::new(Some(fixture_vault("idle", false)));
    app.settings.auto_lock_minutes = 1;
    let mut state = UiState::default();
    // The clock may start at zero: that first frame still counts as the last input.
    drive(&mut app, &mut state, &[], 0.0);
    drive(&mut app, &mut state, &[], 59.5);
    assert!(app.vault.is_some(), "a minute has not passed");
    drive(&mut app, &mut state, &[], 60.0);
    assert!(app.vault.is_none(), "idle for the whole minute: locked");
}

#[test]
fn locking_drops_the_unlocked_hosts() {
    let mut app = NativeApp::new(Some(fixture_vault("lock-drops", true)));
    if let Some(vault) = &app.vault {
        let id = vault.hosts()[0].id;
        app.hosts.edit_host(vault, id);
    }
    assert!(app.hosts.editor.is_some());
    app.lock_vault();
    assert!(app.vault.is_none());
    assert!(
        app.hosts.editor.is_none(),
        "the editor's draft (and password) went too"
    );
}

#[test]
fn settings_is_last_so_it_sits_at_the_bottom() {
    assert!(!crate::native::Section::MAIN.contains(&crate::native::Section::Settings));
    let mut app = NativeApp::new(Some(fixture_vault("nav", false)));
    let mut state = UiState::default();
    let output = drive(&mut app, &mut state, &[], 0.0);
    let logs = zone_center(&output.zones, "Logs");
    let settings = zone_center(&output.zones, "Settings");
    assert!(
        settings.y > logs.y && settings.y > 800.0 - 60.0,
        "{settings:?}"
    );
}

#[test]
fn dragging_a_host_reorders_the_page_and_saves_its_place() {
    let mut app = NativeApp::new(Some(fixture_vault("host-drag", true)));
    let mut state = UiState::default();
    let output = step(&mut app, &mut state, &[], 0.0);
    let from = zone_center(&output.zones, "web-1");
    let to = zone_center(&output.zones, "prod-db");
    let drag = [
        Event::PointerMoved(from),
        Event::PointerDown(Button::Primary),
        Event::PointerMoved(to),
    ];
    let moving = drive(&mut app, &mut state, &drag, 1.0);
    assert!(moving.repaint, "the held card keeps drawing");
    drive(
        &mut app,
        &mut state,
        &[Event::PointerUp(Button::Primary)],
        1.1,
    );
    let labels = || {
        app.vault
            .as_ref()
            .unwrap()
            .hosts()
            .iter()
            .filter(|host| host.group.is_none())
            .map(|host| host.label.as_str())
            .collect::<Vec<_>>()
    };
    assert_eq!(&labels()[..2], ["web-1", "prod-db"]);
    let path = app.vault.as_ref().unwrap().path().to_path_buf();
    drop(app);
    let reopened = annulus_vault::vault::Vault::unlock(&path, "pw").unwrap();
    let saved: Vec<&str> = reopened
        .hosts()
        .iter()
        .filter(|host| host.group.is_none())
        .map(|host| host.label.as_str())
        .collect();
    assert_eq!(&saved[..2], ["web-1", "prod-db"]);
}

/// The panes of the terminal tab in front.
fn front_panes(app: &NativeApp) -> &crate::native::panes::Panes {
    match app
        .tabs
        .get(app.active)
        .and_then(|tab| tab.terminal.as_ref())
    {
        Some(TermView::Live(panes)) => panes,
        _ => panic!("no live terminal tab in front"),
    }
}

#[test]
fn a_new_split_glides_in_with_its_bar_and_settles() {
    use crate::native::panes::{Panes, Replay};

    let mut app = NativeApp::new(Some(fixture_vault("split-motion", false)));
    let pane = Replay {
        title: "shell".into(),
        emulator: crate::native::demo::terminal(),
        status: None,
    };
    app.push_terminal("shell", TermView::Live(Box::new(Panes::replay(pane))));
    let uid = app.tabs[app.active].uid;
    let terminal = |pane: u64| annulus_ui::kit::Id::new(("terminal", uid, pane));
    let rect =
        |output: &FrameOutput, id| output.zones.iter().find(|zone| zone.id == id).unwrap().rect;
    let named = |output: &FrameOutput, name: &str| {
        output
            .zones
            .iter()
            .find(|zone| zone.label == name)
            .unwrap()
            .rect
    };

    let mut state = UiState::default();
    let before = drive(&mut app, &mut state, &[], 0.0);
    let whole = rect(&before, terminal(1));
    drive(&mut app, &mut state, &[chord(shortcuts::SPLIT_RIGHT)], 1.0);
    let moving = drive(&mut app, &mut state, &[], 1.05);
    let settled = drive(&mut app, &mut state, &[], 2.0);
    let old_moving = rect(&moving, terminal(1));
    let old_settled = rect(&settled, terminal(1));
    let new_moving = rect(&moving, terminal(2));
    let new_settled = rect(&settled, terminal(2));
    assert!(
        old_moving.w > old_settled.w && old_moving.w < whole.w,
        "the old pane glides from its old width: {whole:?}, {old_moving:?}, {old_settled:?}"
    );
    assert!(
        new_moving.x > new_settled.x,
        "the new pane slides in from the right: {new_moving:?}, {new_settled:?}"
    );
    assert!(
        named(&moving, "Focus mode").y < named(&settled, "Focus mode").y,
        "the new bar slides down"
    );
    assert!(moving.repaint, "motion asks for its next frame");
    assert!(!settled.repaint, "the split rests once it arrives");
}

#[test]
fn focus_mode_and_broadcast_are_a_click_or_a_shortcut_away() {
    let mut app = NativeApp::new(Some(fixture_vault("split-panes", false)));
    split_tab(&mut app);
    let uid = app.tabs[app.active].uid;
    let terminal = |pane: u64| annulus_ui::kit::Id::new(("terminal", uid, pane));
    let viewport = Rect::new(0.0, 0.0, 1100.0, 800.0);
    let mut state = UiState::default();
    let mut output = step(&mut app, &mut state, &[], 0.0);
    assert_eq!(layout_faults(&output.zones, viewport), Vec::<String>::new());
    // Focus mode: one large pane beside the list; a row brings its pane in front, keyboard
    // and all.
    output = step(&mut app, &mut state, &click_on(&output, "Focus mode"), 1.0);
    assert!(front_panes(&app).in_focus_mode());
    output = step(
        &mut app,
        &mut state,
        &click_on(&output, "bastion, Disconnected or failed"),
        2.0,
    );
    assert_eq!(front_panes(&app).focused, 4);
    assert_eq!(state.focus(), Some(terminal(4)));
    let shown = output
        .zones
        .iter()
        .filter(|zone| zone.role == Role::Terminal)
        .count();
    assert_eq!(shown, 1, "only the large pane is drawn");
    assert_eq!(layout_faults(&output.zones, viewport), Vec::<String>::new());
    // Ctrl+Shift+M goes back to the split.
    output = step(&mut app, &mut state, &[chord(shortcuts::FOCUS_MODE)], 3.0);
    assert!(!front_panes(&app).in_focus_mode());
    // Broadcast types into the other three, until one is left out with its own toggle.
    output = step(&mut app, &mut state, &click_on(&output, "Broadcast"), 4.0);
    assert!(front_panes(&app).broadcasting());
    assert_eq!(front_panes(&app).receivers(), [1, 3, 2]);
    output = step(
        &mut app,
        &mut state,
        &click_on(&output, "Broadcast to web-1"),
        5.0,
    );
    assert_eq!(front_panes(&app).receivers(), [1, 3]);
    let toggle = output
        .zones
        .iter()
        .find(|zone| zone.label == "Broadcast to web-1")
        .unwrap();
    assert_eq!(toggle.toggled, Some(false));
    assert_eq!(
        state.focus(),
        Some(terminal(4)),
        "the keyboard stays with the pane typed in"
    );
    assert_eq!(layout_faults(&output.zones, viewport), Vec::<String>::new());
    // Ctrl+Alt+Shift+B leaves the focused pane (bastion) out by keyboard; another pane typed
    // in then does not reach it, and the same keys take it back.
    let into = |output: &FrameOutput, pane: u64| {
        let zone = output
            .zones
            .iter()
            .find(|zone| zone.id == terminal(pane))
            .unwrap();
        vec![
            Event::PointerMoved(zone.rect.center()),
            Event::PointerDown(Button::Primary),
            Event::PointerUp(Button::Primary),
        ]
    };
    output = step(&mut app, &mut state, &[chord(shortcuts::LEAVE_OUT)], 5.5);
    output = step(&mut app, &mut state, &into(&output, 1), 5.6);
    assert_eq!(front_panes(&app).receivers(), [3], "bastion is left out");
    step(&mut app, &mut state, &into(&output, 4), 5.7);
    output = step(&mut app, &mut state, &[chord(shortcuts::LEAVE_OUT)], 5.8);
    step(&mut app, &mut state, &into(&output, 1), 5.9);
    assert_eq!(front_panes(&app).receivers(), [3, 4], "back in");
    assert!(
        front_panes(&app).broadcasting(),
        "broadcast itself stays on"
    );
    // Ctrl+Shift+B stops it.
    step(&mut app, &mut state, &[chord(shortcuts::BROADCAST)], 6.0);
    assert!(!front_panes(&app).broadcasting());
}

#[test]
fn connecting_or_unlocking_brings_the_accounts_latest_at_once() {
    let mut app = NativeApp::new(Some(fixture_vault("sync-on-connect", true)));
    app.sync.last_round = 5.0;
    let host = app.vault.as_ref().unwrap().hosts()[0].id;
    app.act(crate::native::hosts::HostsAction::Connect(host));
    assert!(
        app.sync.last_round.is_infinite(),
        "a round on the next frame"
    );
    app.sync.last_round = 5.0;
    app.lock_vault();
    assert!(
        app.sync.last_round.is_infinite(),
        "and the first frame after an unlock"
    );
}

#[test]
fn the_theme_browser_previews_cancels_and_applies_without_touching_background_controls() {
    let mut app = NativeApp::new(Some(fixture_vault("theme-browser", true)));
    settings(&mut app);
    app.settings.app_scheme = Some("Solarized Light".into());
    app.settings.theme = ThemeMode::Light;
    app.settings.reduce_motion = true;
    let mut state = UiState::default();
    let output = step(&mut app, &mut state, &[], 0.0);
    let output = step(&mut app, &mut state, &click_on(&output, "Appearance"), 1.0);
    let output = step(
        &mut app,
        &mut state,
        &click_on(&output, "Solarized Light"),
        2.0,
    );
    assert!(app.settings_view.picker.is_some());
    assert!(
        !output
            .zones
            .iter()
            .any(|zone| zone.label == "Reduce motion")
    );
    let output = step(&mut app, &mut state, &click_on(&output, "All"), 3.0);
    // Return focus to search after using the filter, as a pointer user would.
    let search = output
        .zones
        .iter()
        .find(|zone| zone.id == crate::native::Id::new("theme.search"))
        .unwrap()
        .rect
        .center();
    step(
        &mut app,
        &mut state,
        &[
            Event::PointerMoved(search),
            Event::PointerDown(Button::Primary),
            Event::PointerUp(Button::Primary),
        ],
        4.0,
    );
    let output = step(&mut app, &mut state, &[Event::Text("Dracula".into())], 5.0);
    assert!(
        output
            .zones
            .iter()
            .any(|zone| zone.label == "Dracula" && zone.role == Role::ListItem)
    );
    assert_eq!(app.settings.app_scheme.as_deref(), Some("Solarized Light"));
    assert_eq!(app.settings.recent_themes, Vec::<String>::new());
    let output = step(
        &mut app,
        &mut state,
        &[Event::Key(Key::Escape, Modifiers::NONE)],
        6.0,
    );
    assert!(app.settings_view.picker.is_none());
    assert_eq!(app.settings.app_scheme.as_deref(), Some("Solarized Light"));
    assert_eq!(state.focus(), Some(crate::native::Id::new("settings.scheme")));
    let output = step(
        &mut app,
        &mut state,
        &click_on(&output, "Solarized Light"),
        7.0,
    );
    let output = step(&mut app, &mut state, &click_on(&output, "All"), 8.0);
    let search = output
        .zones
        .iter()
        .find(|zone| zone.id == crate::native::Id::new("theme.search"))
        .unwrap()
        .rect
        .center();
    step(
        &mut app,
        &mut state,
        &[
            Event::PointerMoved(search),
            Event::PointerDown(Button::Primary),
            Event::PointerUp(Button::Primary),
        ],
        9.0,
    );
    step(&mut app, &mut state, &[Event::Text("Dracula".into())], 10.0);
    step(
        &mut app,
        &mut state,
        &[Event::Key(Key::Enter, Modifiers::NONE)],
        11.0,
    );
    assert!(app.settings_view.picker.is_none());
    assert_eq!(app.settings.app_scheme.as_deref(), Some("Dracula"));
    assert_eq!(app.settings.theme, ThemeMode::Dark);
    assert_eq!(app.settings.recent_themes, ["Dracula"]);
}

#[test]
fn app_and_terminal_font_choices_are_independent_and_remembered() {
    let mut app = NativeApp::new(Some(fixture_vault("font-families", true)));
    settings(&mut app);
    app.settings.reduce_motion = true;
    let mut state = UiState::default();
    let output = step(&mut app, &mut state, &[], 0.0);
    let output = step(&mut app, &mut state, &click_on(&output, "Appearance"), 1.0);
    let output = step(
        &mut app,
        &mut state,
        &click_on(&output, "System default"),
        2.0,
    );
    let output = step(&mut app, &mut state, &click_on(&output, "Fira Mono"), 3.0);
    assert_eq!(app.settings.ui_font.as_deref(), Some("Fira Mono"));
    assert_eq!(app.settings.terminal_font, None);
    let output = step(&mut app, &mut state, &click_on(&output, "Terminal"), 4.0);
    let output = step(
        &mut app,
        &mut state,
        &click_on(&output, "JetBrains Mono (default)"),
        5.0,
    );
    step(&mut app, &mut state, &click_on(&output, "Fira Mono"), 6.0);
    assert_eq!(app.settings.terminal_font.as_deref(), Some("Fira Mono"));
    assert_eq!(app.settings.ui_font.as_deref(), Some("Fira Mono"));
}

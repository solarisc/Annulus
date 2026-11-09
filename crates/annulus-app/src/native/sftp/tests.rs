//! SFTP view and file-operation tests.

#![allow(clippy::unwrap_used, clippy::panic)]

use annulus_config::tokens::Appearance;
use annulus_platform::cpu::{CpuCanvas, TextEngine};
use annulus_session::{Hop, SshFiles, Target};
use annulus_ui::kit::{Theme, UiState};

use super::*;

/// Draws `view` once with `events` at `time`; returns the frame's zones.
fn frame(
    view: &mut SftpView,
    state: &mut UiState,
    events: &[annulus_ui::kit::Event],
    time: f64,
) -> Vec<annulus_ui::kit::Zone> {
    let mut canvas = CpuCanvas::new(1100, 680, 1.0, TextEngine::new()).unwrap();
    state.begin_frame(events, time);
    let viewport = Rect::new(0.0, 0.0, 1100.0, 680.0);
    let mut ui = Ui::new(&mut canvas, Theme::new(Appearance::Dark), state, viewport);
    view.draw(&mut ui, viewport, None);
    // Menus are drawn over everything, as the window does.
    ui.overlays();
    state.end_frame().zones
}

fn click(zones: &[annulus_ui::kit::Zone], name: &str) -> Vec<annulus_ui::kit::Event> {
    use annulus_ui::kit::{Button, Event};
    let zone = zones
        .iter()
        .rev()
        .find(|zone| zone.label == name)
        .unwrap_or_else(|| panic!("no zone {name:?}"));
    vec![
        Event::PointerMoved(zone.rect.center()),
        Event::PointerDown(Button::Primary),
        Event::PointerUp(Button::Primary),
    ]
}

/// Draws until a local file request has finished, or fails instead of racing its worker.
fn wait_for_local(view: &mut SftpView, state: &mut UiState, time: f64) {
    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while view.tab.local.loading {
        frame(view, state, &[], time);
        assert!(
            std::time::Instant::now() < until,
            "the local file worker did not finish"
        );
        std::thread::yield_now();
    }
}

/// Drags the row named `from` and lets it go over the row named `onto`.
fn drag(view: &mut SftpView, state: &mut UiState, from: &str, onto: &str, time: f64) {
    use annulus_ui::kit::{Button, Event};
    let zones = frame(view, state, &[], time);
    let at = |name: &str| {
        zones
            .iter()
            .find(|zone| zone.label == name)
            .unwrap_or_else(|| panic!("no row {name:?}"))
            .rect
            .center()
    };
    let (start, end) = (at(from), at(onto));
    let moving = [
        Event::PointerMoved(start),
        Event::PointerDown(Button::Primary),
        Event::PointerMoved(end),
    ];
    frame(view, state, &moving, time + 0.1);
    frame(
        view,
        state,
        &[Event::PointerUp(Button::Primary)],
        time + 0.2,
    );
}

#[test]
fn a_file_dragged_onto_the_other_pane_is_uploaded_or_downloaded() {
    let mut view = SftpView::new(crate::native::demo::sftp_tab("drag"));
    let mut state = UiState::default();
    drag(&mut view, &mut state, "todo.txt", "README.md", 0.0);
    let labels = |view: &SftpView| -> Vec<String> {
        view.tab
            .transfers
            .iter()
            .map(|transfer| transfer.label.clone())
            .collect()
    };
    assert_eq!(labels(&view), ["\u{2191} todo.txt"], "local onto the host");
    drag(&mut view, &mut state, "deploy.sh", "backup.tar.gz", 1.0);
    assert_eq!(
        labels(&view),
        ["\u{2191} todo.txt", "\u{2193} deploy.sh"],
        "the host's onto this computer"
    );
    // Let go over its own pane, nothing moves.
    drag(&mut view, &mut state, "app.log", "README.md", 2.0);
    assert_eq!(view.tab.transfers.len(), 2);
    // A folder from this computer goes with everything in it.
    drag(&mut view, &mut state, "notes", "README.md", 3.0);
    assert_eq!(labels(&view)[2], "\u{2191} notes/");
    // And one from the host comes down whole.
    drag(&mut view, &mut state, "config", "todo.txt", 4.0);
    assert_eq!(labels(&view)[3], "\u{2193} config/");
}

#[test]
fn a_right_click_on_a_file_opens_its_menu_in_either_pane() {
    let mut view = SftpView::new(crate::native::demo::sftp_tab("right-click"));
    let mut state = UiState::default();
    let zones = frame(&mut view, &mut state, &[], 0.0);
    let right_click = |zones: &[annulus_ui::kit::Zone], name: &str| {
        let at = zones
            .iter()
            .find(|zone| zone.label == name)
            .unwrap()
            .rect
            .center();
        vec![
            annulus_ui::kit::Event::PointerMoved(at),
            annulus_ui::kit::Event::PointerDown(annulus_ui::kit::Button::Secondary),
            annulus_ui::kit::Event::PointerUp(annulus_ui::kit::Button::Secondary),
        ]
    };
    let items = |zones: &[annulus_ui::kit::Zone]| -> Vec<String> {
        zones
            .iter()
            .filter(|zone| zone.role == Role::MenuItem)
            .map(|zone| zone.label.clone())
            .collect()
    };
    // On the host: its whole menu.
    let zones = frame(&mut view, &mut state, &right_click(&zones, "app.log"), 1.0);
    let menu = items(&zones);
    assert!(menu.contains(&"Download".to_owned()) && menu.contains(&"Rename".to_owned()));
    // Choosing from it acts like the More menu: Download starts the download.
    let zones = frame(&mut view, &mut state, &click(&zones, "Download"), 2.0);
    assert_eq!(view.tab.transfers.len(), 1);
    // On this computer: what a file manager offers, and up to the host.
    let zones = frame(&mut view, &mut state, &right_click(&zones, "todo.txt"), 3.0);
    let menu = items(&zones);
    for item in [
        "Open",
        "Upload to the host",
        "Rename",
        "Copy path",
        "Properties",
        "Delete",
    ] {
        assert!(menu.contains(&item.to_owned()), "{item} in {menu:?}");
    }
    assert!(
        menu.iter().any(|item| item.starts_with("Show in ")),
        "{menu:?}"
    );
    frame(
        &mut view,
        &mut state,
        &click(&zones, "Upload to the host"),
        4.0,
    );
    assert_eq!(view.tab.transfers.len(), 2);
}

/// The labels of `view`'s transfers.
fn labels(view: &SftpView) -> Vec<String> {
    view.tab
        .transfers
        .iter()
        .map(|transfer| transfer.label.clone())
        .collect()
}

/// The name and policy a transfer's recipe lands with.
fn landing(view: &SftpView, at: usize) -> (Option<String>, Conflict) {
    use crate::sftp_view::Recipe;
    match &view.tab.transfers[at].recipe {
        Recipe::Upload { name, conflict, .. }
        | Recipe::UploadTree { name, conflict, .. }
        | Recipe::Download { name, conflict, .. }
        | Recipe::DownloadTree { name, conflict, .. } => (name.clone(), *conflict),
    }
}

#[test]
fn a_name_taken_where_a_file_goes_asks_to_skip_replace_or_rename_it() {
    let mut view = SftpView::new(crate::native::demo::sftp_tab("clash"));
    // The host has README.md already; so does this computer have deploy.sh.
    std::fs::write(view.tab.local.dir.join("README.md"), "mine").unwrap();
    std::fs::write(view.tab.local.dir.join("deploy.sh"), "mine").unwrap();
    view.tab.local = crate::local_pane::LocalPane::new_loaded(view.tab.local.dir.clone());
    let mut state = UiState::default();
    drag(&mut view, &mut state, "README.md", "app.log", 0.0);
    assert!(
        view.tab.transfers.is_empty(),
        "nothing goes before the answer"
    );
    let zones = frame(&mut view, &mut state, &[], 1.0);
    for button in ["Skip", "Replace", "Rename"] {
        assert!(zones.iter().any(|zone| zone.label == button), "{button}");
    }
    // Rename keeps both, under a free name to start with.
    assert_eq!(view.clashes[0].rename, "README (1).md");
    frame(&mut view, &mut state, &click(&zones, "Rename"), 2.0);
    assert_eq!(labels(&view), ["\u{2191} README (1).md"]);
    assert_eq!(
        landing(&view, 0),
        (Some("README (1).md".into()), Conflict::KeepBoth)
    );
    // Replace sends it under its own name, over what is there.
    drag(&mut view, &mut state, "README.md", "app.log", 3.0);
    let zones = frame(&mut view, &mut state, &[], 4.0);
    frame(&mut view, &mut state, &click(&zones, "Replace"), 5.0);
    assert_eq!(landing(&view, 1), (None, Conflict::Overwrite));
    // Skip sends nothing.
    drag(&mut view, &mut state, "README.md", "app.log", 6.0);
    let zones = frame(&mut view, &mut state, &[], 7.0);
    frame(&mut view, &mut state, &click(&zones, "Skip"), 8.0);
    assert_eq!(view.tab.transfers.len(), 2);
    assert!(view.clashes.is_empty());
    // A download onto a name taken on this computer asks the same (deploy.sh is in both
    // panes now, so it is sent as its Download item would).
    let host_copy = view
        .tab
        .entries
        .iter()
        .find(|e| e.name == "deploy.sh")
        .cloned();
    view.send_down(host_copy.unwrap());
    assert!(view.clashes.len() == 1 && view.tab.transfers.len() == 2);
    // A name typed for Rename that is taken too is refused.
    view.clashes[0].rename = "todo.txt".into();
    let zones = frame(&mut view, &mut state, &[], 10.0);
    frame(&mut view, &mut state, &click(&zones, "Rename"), 11.0);
    assert_eq!(view.clashes.len(), 1, "still asking");
    // Without asking, the tab's policy decides as before.
    view.clashes.clear();
    view.tab.ask_on_clash = false;
    drag(&mut view, &mut state, "README.md", "app.log", 12.0);
    assert_eq!(view.tab.transfers.len(), 3);
    assert_eq!(landing(&view, 2), (None, Conflict::KeepBoth));
}

#[test]
fn one_answer_can_go_for_every_clash_waiting() {
    let mut view = SftpView::new(crate::native::demo::sftp_tab("clash-all"));
    let dir = view.tab.local.dir.clone();
    for name in ["README.md", "app.log"] {
        std::fs::write(dir.join(name), "mine").unwrap();
    }
    let mut state = UiState::default();
    frame(&mut view, &mut state, &[], 0.0);
    let dropped = annulus_ui::kit::Event::FilesDropped(
        vec![
            dir.join("README.md"),
            dir.join("app.log"),
            dir.join("todo.txt"),
        ],
        None,
    );
    frame(&mut view, &mut state, &[dropped], 1.0);
    assert_eq!(labels(&view), ["\u{2191} todo.txt"], "no clash: it goes");
    let zones = frame(&mut view, &mut state, &[], 2.0);
    let all = "Do the same for the 1 other waiting";
    let zones = frame(&mut view, &mut state, &click(&zones, all), 3.0);
    frame(&mut view, &mut state, &click(&zones, "Rename"), 4.0);
    assert_eq!(
        labels(&view),
        [
            "\u{2191} todo.txt",
            "\u{2191} README (1).md",
            "\u{2191} app (1).log"
        ],
        "each takes a free name of its own"
    );
    assert!(view.clashes.is_empty());
}

#[test]
fn files_from_the_os_let_go_over_this_computer_are_copied_into_its_folder() {
    let mut view = SftpView::new(crate::native::demo::sftp_tab("copy-in"));
    let source = crate::native::demo::scratch("sftp", "copy-in-source");
    let _ = std::fs::remove_dir_all(&source);
    std::fs::create_dir_all(source.join("photos")).unwrap();
    std::fs::write(source.join("photos/cat.jpg"), "meow").unwrap();
    std::fs::write(source.join("todo.txt"), "theirs").unwrap();
    let mut state = UiState::default();
    let zones = frame(&mut view, &mut state, &[], 0.0);
    let here = zones
        .iter()
        .find(|zone| zone.label == "backup.tar.gz")
        .unwrap()
        .rect
        .center();
    let dropped = annulus_ui::kit::Event::FilesDropped(
        vec![source.join("photos"), source.join("todo.txt")],
        Some(here),
    );
    frame(&mut view, &mut state, &[dropped], 1.0);
    assert!(view.tab.transfers.is_empty(), "nothing goes to the host");
    // todo.txt is here already: it asks; the folder is copied meanwhile.
    assert_eq!(view.clashes.len(), 1);
    let zones = frame(&mut view, &mut state, &[], 2.0);
    frame(&mut view, &mut state, &click(&zones, "Replace"), 3.0);
    let dir = view.tab.local.dir.clone();
    let started = std::time::Instant::now();
    while std::fs::read_to_string(dir.join("todo.txt")).unwrap() != "theirs"
        || !dir.join("photos/cat.jpg").exists()
    {
        assert!(started.elapsed().as_secs() < 10, "the copies never landed");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(
        std::fs::read_to_string(dir.join("photos/cat.jpg")).unwrap(),
        "meow"
    );
    assert!(
        !local_names(&dir)
            .iter()
            .any(|name| name.ends_with(".annulus-part")),
        "no part file is left"
    );
}

#[test]
fn local_files_with_the_same_name_wait_for_a_choice_and_both_are_kept() {
    let root = crate::native::demo::scratch("sftp", "same-name-copy");
    let _ = std::fs::remove_dir_all(&root);
    let first_dir = root.join("first");
    let second_dir = root.join("second");
    let target = root.join("target");
    std::fs::create_dir_all(&first_dir).unwrap();
    std::fs::create_dir_all(&second_dir).unwrap();
    std::fs::create_dir_all(&target).unwrap();
    let first = first_dir.join("notes.txt");
    let second = second_dir.join("notes.txt");
    std::fs::write(&first, "first").unwrap();
    std::fs::write(&second, "second").unwrap();

    let mut view = SftpView::new(crate::native::demo::sftp_tab("same-name-copy"));
    view.tab.local = crate::local_pane::LocalPane::new(target.clone());
    view.copy_in(first);
    view.copy_in(second);

    assert_eq!(view.copying.len(), 1);
    assert_eq!(view.clashes.len(), 1);
    let first_done = view
        .copies
        .1
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    assert!(first_done.result.is_ok());
    view.copying.remove(&first_done.target);

    view.answer_clash(&Answer::Rename("notes second.txt".into()));
    let second_done = view
        .copies
        .1
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    assert!(second_done.result.is_ok());
    view.copying.remove(&second_done.target);

    assert_eq!(
        std::fs::read_to_string(target.join("notes.txt")).unwrap(),
        "first"
    );
    assert_eq!(
        std::fs::read_to_string(target.join("notes second.txt")).unwrap(),
        "second"
    );
    assert!(view.clashes.is_empty());
    assert!(view.copying.is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_rejected_duplicate_copy_does_not_release_the_copy_already_using_that_name() {
    let root = crate::native::demo::scratch("sftp", "copy-reservation");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let source = root.join("source.txt");
    let target = root.join("target.txt");
    std::fs::write(&source, "source").unwrap();

    let mut view = SftpView::new(crate::native::demo::sftp_tab("copy-reservation"));
    view.copying.insert(target.clone());
    view.copy_now(source, target.clone(), false);
    let rejected = view
        .copies
        .1
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    assert!(!rejected.release);
    if rejected.release {
        view.copying.remove(&rejected.target);
    }
    assert!(view.copying.contains(&target));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn replacing_a_local_file_keeps_the_old_one_until_the_new_copy_is_complete() {
    let root = crate::native::demo::scratch("sftp", "replace-local");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let source = root.join("source.txt");
    let target = root.join("target.txt");
    std::fs::write(&source, "new").unwrap();
    std::fs::write(&target, "old").unwrap();

    copy_into(&source, &target, true).unwrap();

    assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
    assert!(
        local_names(&root)
            .iter()
            .all(|name| !name.contains(".annulus-")),
        "no part or old file is left"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_drag_offers_the_panes_that_take_it() {
    let row = |dragged| Some((dragged, Point::new(1.0, 1.0)));
    let local = Dragged::Local(PathBuf::from("a.txt"));
    let remote = Dragged::Remote(Entry {
        name: "b.txt".into(),
        kind: annulus_sftp::Kind::File,
        size: 1,
        modified: None,
        permissions: None,
        uid: None,
        gid: None,
        accessed: None,
    });
    assert_eq!(
        drop_targets(row(local).as_ref(), FromOs::No),
        [Side::Remote]
    );
    assert_eq!(
        drop_targets(row(remote).as_ref(), FromOs::No),
        [Side::Local]
    );
    // From the OS: either pane where the OS says where it is; else the host's.
    let at = FromOs::At(Point::new(5.0, 5.0));
    assert_eq!(drop_targets(None, at), [Side::Local, Side::Remote]);
    assert_eq!(drop_targets(None, FromOs::Somewhere), [Side::Remote]);
    assert_eq!(drop_targets(None, FromOs::No), Vec::<Side>::new());
}

#[test]
fn the_listings_show_permissions_and_a_right_click_on_the_headers_picks_columns() {
    let mut view = SftpView::new(crate::native::demo::sftp_tab("columns"));
    let mut state = UiState::default();
    let zones = frame(&mut view, &mut state, &[], 0.0);
    let has = |zones: &[annulus_ui::kit::Zone], name: &str| {
        zones.iter().filter(|zone| zone.label == name).count()
    };
    assert_eq!(has(&zones, "Sort by permissions"), 2, "in both panes");
    let header = zones
        .iter()
        .rev()
        .find(|zone| zone.label == "Sort by modified")
        .unwrap()
        .rect
        .center();
    let right = [
        annulus_ui::kit::Event::PointerMoved(header),
        annulus_ui::kit::Event::PointerDown(annulus_ui::kit::Button::Secondary),
        annulus_ui::kit::Event::PointerUp(annulus_ui::kit::Button::Secondary),
    ];
    let zones = frame(&mut view, &mut state, &right, 1.0);
    let menu: Vec<&str> = zones
        .iter()
        .filter(|zone| zone.role == Role::MenuItem)
        .map(|zone| zone.label.as_str())
        .collect();
    assert!(
        menu.contains(&"Hide Permissions") && menu.contains(&"Show Group"),
        "{menu:?}"
    );
    assert!(
        menu.contains(&"Hide Owner (no room for it now)"),
        "shown, but this window is too narrow: {menu:?}"
    );
    assert!(
        !menu.contains(&"Show Created"),
        "the host keeps no creation time"
    );
    frame(
        &mut view,
        &mut state,
        &click(&zones, "Hide Permissions"),
        2.0,
    );
    let zones = frame(&mut view, &mut state, &[], 3.0);
    assert_eq!(
        has(&zones, "Sort by permissions"),
        1,
        "gone from the host's pane only"
    );
    let (local, remote) = view.take_columns().unwrap();
    assert!(local.contains(&"permissions".to_owned()));
    assert!(!remote.contains(&"permissions".to_owned()));
    assert_eq!(view.take_columns(), None, "told once");
    // A column asked for comes first when room runs short, so it shows.
    let header = zones
        .iter()
        .rev()
        .find(|zone| zone.label == "Sort by modified")
        .unwrap()
        .rect
        .center();
    let right = [
        annulus_ui::kit::Event::PointerMoved(header),
        annulus_ui::kit::Event::PointerDown(annulus_ui::kit::Button::Secondary),
        annulus_ui::kit::Event::PointerUp(annulus_ui::kit::Button::Secondary),
    ];
    let zones = frame(&mut view, &mut state, &right, 4.0);
    frame(&mut view, &mut state, &click(&zones, "Show Group"), 5.0);
    let zones = frame(&mut view, &mut state, &[], 6.0);
    assert_eq!(has(&zones, "Sort by group"), 1);
    // The host's permissions read as a file manager shows them.
    let row = Row::remote(&view.tab.entries[4], &Owners::default());
    assert_eq!(row.cell(Column::Permissions), "rw-r-----");
}

#[test]
fn this_computers_files_are_made_renamed_and_deleted_after_a_yes() {
    let mut view = SftpView::new(crate::native::demo::sftp_tab("local-ops"));
    let dir = view.tab.local.dir.clone();
    let mut state = UiState::default();
    let zones = frame(&mut view, &mut state, &[], 0.0);
    // New folder from the empty space's menu.
    let blank = zones
        .iter()
        .find(|zone| zone.id == Id::new(("sftp.blank", true)))
        .unwrap()
        .rect;
    let below = Point::new(blank.center().x, blank.b() - 10.0);
    let right = [
        annulus_ui::kit::Event::PointerMoved(below),
        annulus_ui::kit::Event::PointerDown(annulus_ui::kit::Button::Secondary),
        annulus_ui::kit::Event::PointerUp(annulus_ui::kit::Button::Secondary),
    ];
    let zones = frame(&mut view, &mut state, &right, 1.0);
    frame(&mut view, &mut state, &click(&zones, "New folder"), 2.0);
    let zones = frame(&mut view, &mut state, &[], 3.0);
    let mut typing = click(&zones, "Folder name");
    typing.push(annulus_ui::kit::Event::Text("drafts".into()));
    frame(&mut view, &mut state, &typing, 4.0);
    let enter = [annulus_ui::kit::Event::Key(Key::Enter, Modifiers::NONE)];
    frame(&mut view, &mut state, &enter, 5.0);
    wait_for_local(&mut view, &mut state, 5.1);
    assert!(dir.join("drafts").is_dir());
    // F2 on a selected file renames it.
    let zones = frame(&mut view, &mut state, &[], 6.0);
    frame(&mut view, &mut state, &click(&zones, "todo.txt"), 7.0);
    let f2 = [annulus_ui::kit::Event::Key(Key::F(2), Modifiers::NONE)];
    frame(&mut view, &mut state, &f2, 8.0);
    if let Some((Side::Local, Edit::Rename { to, .. })) = &mut view.edit {
        *to = "done.txt".into();
    } else {
        panic!("F2 did not start a rename here");
    }
    let zones = frame(&mut view, &mut state, &[], 9.0);
    frame(&mut view, &mut state, &click(&zones, "Save"), 10.0);
    wait_for_local(&mut view, &mut state, 10.1);
    assert!(dir.join("done.txt").exists() && !dir.join("todo.txt").exists());
    // Delete asks first; only the card's Delete removes it.
    let zones = frame(&mut view, &mut state, &[], 11.0);
    frame(&mut view, &mut state, &click(&zones, "done.txt"), 12.0);
    let delete = [annulus_ui::kit::Event::Key(Key::Delete, Modifiers::NONE)];
    let zones = frame(&mut view, &mut state, &delete, 13.0);
    let zones = if zones.iter().any(|zone| zone.label == "Delete") {
        zones
    } else {
        frame(&mut view, &mut state, &[], 14.0)
    };
    assert!(
        dir.join("done.txt").exists(),
        "nothing is deleted before the yes"
    );
    frame(&mut view, &mut state, &click(&zones, "Delete"), 15.0);
    wait_for_local(&mut view, &mut state, 15.1);
    assert!(!dir.join("done.txt").exists());
}

#[test]
fn files_dropped_from_the_os_upload_to_the_folder_shown() {
    let mut view = SftpView::new(crate::native::demo::sftp_tab("os-drop"));
    let file = view.tab.local.dir.join("todo.txt");
    let mut state = UiState::default();
    frame(&mut view, &mut state, &[], 0.0);
    frame(
        &mut view,
        &mut state,
        &[annulus_ui::kit::Event::FilesDropped(vec![file], None)],
        1.0,
    );
    assert_eq!(view.tab.transfers.len(), 1);
    assert!(view.tab.transfers[0].upload);
}

#[test]
fn a_rename_started_in_one_folder_ends_when_another_is_shown() {
    let mut tab = SftpTab::unavailable("files", "offline");
    tab.cwd = Some("/home/a".into());
    tab.failed = None;
    let mut view = SftpView::new(tab);
    view.edit = Some((
        Side::Remote,
        Edit::Rename {
            from: "notes.txt".into(),
            to: "notes.txt".into(),
        },
    ));
    view.edit_dir = "/home/a".into();
    let mut state = UiState::default();
    frame(&mut view, &mut state, &[], 0.0);
    assert!(view.edit.is_some(), "same folder: still renaming");
    view.tab.cwd = Some("/home/a/sub".into());
    frame(&mut view, &mut state, &[], 1.0);
    assert!(view.edit.is_none());
}

/// End to end on test VM 1: a folder made with New folder appears, and Delete removes
/// it only after the card's yes.
#[test]
#[ignore = "needs the test servers"]
fn e2e_the_new_sftp_tab_makes_a_folder_and_deletes_it_after_a_yes() {
    use annulus_ui::kit::Event;
    let var = |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("{name} unset"));
    let dir = std::env::temp_dir().join(format!("annulus-native-sftp-rm-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let hop = Hop {
        target: Target {
            host: var("ANNULUS_E2E_VM1"),
            port: 22,
            user: var("ANNULUS_E2E_USER"),
        },
        password: None,
        identity: Some(annulus_session::Password::new(
            &std::fs::read_to_string(var("ANNULUS_E2E_KEY")).unwrap(),
        )),
        proxy: None,
    };
    let files = SshFiles {
        known_hosts: dir.join("known_hosts"),
        key_dir: None,
    };
    let mut tab = SftpTab::new("test-1 files", hop, files, Vec::new());
    tab.local.dir.clone_from(&dir);
    let mut view = SftpView::new(tab);
    let mut state = UiState::default();
    let started = std::time::Instant::now();
    let clock = || started.elapsed().as_secs_f64();
    let name = format!("annulus-e2e-rm-{}", std::process::id());
    let has = |view: &SftpView| view.tab.entries.iter().any(|entry| entry.name == name);
    // Connected, the home folder listed.
    let mut zones = frame(&mut view, &mut state, &[], clock());
    while view.tab.cwd.is_none() || view.tab.entries.is_empty() {
        assert!(clock() < 60.0, "no listing; failed: {:?}", view.tab.failed);
        if matches!(view.ask, Some(Ask::HostKey(_))) {
            view.tab.answers().unwrap().host_key(true);
            view.ask = None;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
        zones = frame(&mut view, &mut state, &[], clock());
    }
    // New folder, typed and saved with Enter.
    frame(&mut view, &mut state, &click(&zones, "New folder"), clock());
    zones = frame(&mut view, &mut state, &[], clock());
    let mut typing = click(&zones, "Folder name");
    typing.push(Event::Text(name.clone()));
    frame(&mut view, &mut state, &typing, clock());
    let enter = [Event::Key(annulus_ui::kit::Key::Enter, Modifiers::NONE)];
    zones = frame(&mut view, &mut state, &enter, clock());
    while !has(&view) {
        assert!(
            clock() < 90.0,
            "the folder never appeared: {:?}",
            view.tab.list_error
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
        zones = frame(&mut view, &mut state, &[], clock());
    }
    // Delete asks; only the card's Delete removes it.
    frame(&mut view, &mut state, &click(&zones, &name), clock());
    let delete = [Event::Key(annulus_ui::kit::Key::Delete, Modifiers::NONE)];
    frame(&mut view, &mut state, &delete, clock());
    zones = frame(&mut view, &mut state, &[], clock());
    assert!(matches!(view.tab.dialog, Some(Dialog::Delete(_))));
    assert!(has(&view), "nothing is deleted before the yes");
    frame(&mut view, &mut state, &click(&zones, "Delete"), clock());
    while has(&view) {
        assert!(
            clock() < 120.0,
            "the folder was not deleted: {:?}",
            view.tab.list_error
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
        frame(&mut view, &mut state, &[], clock());
    }
}

/// End to end on test VM 1: the new UI's SFTP tab connects, trusts the new host key
/// through the same answer the card sends, lists the home folder, and is photographed.
#[test]
#[ignore = "needs the test servers"]
fn e2e_the_new_sftp_tab_lists_a_real_server() {
    let var = |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("{name} unset"));
    let dir = std::env::temp_dir().join(format!("annulus-native-sftp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("local-file.txt"), "hello").unwrap();
    let hop = Hop {
        target: Target {
            host: var("ANNULUS_E2E_VM1"),
            port: 22,
            user: var("ANNULUS_E2E_USER"),
        },
        password: None,
        identity: Some(annulus_session::Password::new(
            &std::fs::read_to_string(var("ANNULUS_E2E_KEY")).unwrap(),
        )),
        proxy: None,
    };
    let files = SshFiles {
        known_hosts: dir.join("known_hosts"),
        key_dir: None,
    };
    let mut tab = SftpTab::new("test-1 files", hop, files, Vec::new());
    tab.local.dir.clone_from(&dir);
    let mut view = SftpView::new(tab);
    let mut state = UiState::default();
    let mut text = Some(TextEngine::new());
    let started = std::time::Instant::now();
    let viewport = Rect::new(0.0, 0.0, 1100.0, 680.0);
    loop {
        assert!(
            started.elapsed() < std::time::Duration::from_secs(60),
            "no listing; failed: {:?}",
            view.tab.failed
        );
        let mut canvas = CpuCanvas::new(1100, 680, 1.0, text.take().unwrap()).unwrap();
        canvas.clear(Theme::new(Appearance::Dark).bg);
        state.begin_frame(&[], started.elapsed().as_secs_f64());
        let mut ui = Ui::new(
            &mut canvas,
            Theme::new(Appearance::Dark),
            &mut state,
            viewport,
        );
        view.draw(&mut ui, viewport, None);
        state.end_frame();
        if matches!(view.ask, Some(Ask::HostKey(_))) {
            view.tab.answers().unwrap().host_key(true);
            view.ask = None;
        }
        assert_eq!(view.tab.failed, None);
        if view.tab.cwd.is_some() && !view.tab.entries.is_empty() {
            let shots = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/tmp/scenes");
            std::fs::create_dir_all(&shots).unwrap();
            std::fs::write(
                shots.join("sftp-live-vm1.png"),
                canvas.encode_png().unwrap(),
            )
            .unwrap();
            return;
        }
        text = Some(canvas.into_text_engine());
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

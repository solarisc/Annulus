//! Agent transfer guards, approval, cancellation and control replies.

use super::*;
use crate::native::demo::fixture_vault;

#[test]
fn scp_calls_wait_for_completion_and_return_bytes_and_cleanup_through_the_door() {
    use annulus_control::client::Client;
    let mut app = NativeApp::new(Some(fixture_vault("scp-door", true)));
    app.vault_path = app.vault.as_ref().map(|v| v.path().to_path_buf());
    let dir = app
        .vault_path
        .as_ref()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    app.waker = Some(std::sync::Arc::new(|| {}));
    app.settings.automation = true;
    app.settings.approve_commands = true;
    let mut request = request();
    request.host = app.vault.as_ref().unwrap().hosts()[0].id;
    app.control();
    let caller = std::thread::spawn(move || {
        let mut client = Client::connect(&dir).unwrap();
        let copied = client.scp(request.clone()).unwrap();
        request.host = u64::MAX;
        let refused = client.scp(request).unwrap_err().to_string();
        (copied, refused)
    });
    let deadline = Instant::now() + std::time::Duration::from_secs(10);
    let mut finished = false;
    while !caller.is_finished() && Instant::now() < deadline {
        app.control();
        if let Some(transfer) = app.transfers.first() {
            let tab = transfer.tab;
            if let Some(approval) = transfer.approval {
                assert!(!caller.is_finished(), "the client waits before approval");
                let files = &mut app
                    .tabs
                    .iter_mut()
                    .find(|t| t.uid == tab)
                    .unwrap()
                    .sftp
                    .as_mut()
                    .unwrap()
                    .tab;
                // The byte-copy tests use an SFTP server. This test supplies its completion
                // to check the real control connection, dispatch and result lifecycle.
                files.start = None;
                files.cwd = Some("/tmp".into());
                app.decide_approval(approval, true);
            } else if transfer.transfer.is_some() && !finished {
                assert!(!caller.is_finished(), "queuing is not success");
                let files = &mut app
                    .tabs
                    .iter_mut()
                    .find(|t| t.uid == tab)
                    .unwrap()
                    .sftp
                    .as_mut()
                    .unwrap()
                    .tab;
                files.transfers[0].state = TransferState::Done("/tmp/agent file.txt".into());
                files.transfers[0].done = 1234;
                finished = true;
            }
        }
        std::thread::yield_now();
    }
    assert!(caller.is_finished(), "the control call must finish");
    let (copied, refused) = caller.join().unwrap();
    assert_eq!(copied.bytes, 1234);
    assert_eq!(copied.to, "/tmp/agent file.txt");
    assert_eq!(copied.cleanup, TRANSFER_CLEANUP);
    assert!(
        app.tabs
            .iter()
            .any(|tab| tab.uid == copied.tab && tab.agent.is_some())
    );
    assert!(refused.contains("no saved host"));
    assert!(refused.contains(TRANSFER_CLEANUP));
    assert!(app.transfers.is_empty());
}

fn request() -> TransferRequest {
    TransferRequest {
        host: 1,
        upload: true,
        local: std::env::temp_dir()
            .join("agent file.txt")
            .display()
            .to_string(),
        remote: "/tmp/agent file.txt".into(),
        recursive: false,
        overwrite: false,
    }
}

#[test]
fn agent_copies_require_access_a_known_host_and_safe_exact_paths() {
    let mut app = NativeApp::new(Some(fixture_vault("scp-guards", true)));
    let mut request = request();
    assert!(app.prepare_transfer(&request).is_err());
    app.settings.automation = true;
    request.host = u64::MAX;
    assert!(app.prepare_transfer(&request).is_err());
    assert!(app.transfers.is_empty());
    request.local = "relative.txt".into();
    assert!(recipe(&request).is_err());
    request = self::request();
    request.remote = "/tmp/bad\npath".into();
    assert!(recipe(&request).is_err());
    request = self::request();
    assert!(matches!(
        recipe(&request).unwrap(),
        Recipe::Upload {
            conflict: Conflict::Refuse,
            ..
        }
    ));
    request.upload = false;
    request.recursive = true;
    request.overwrite = true;
    assert!(matches!(
        recipe(&request).unwrap(),
        Recipe::DownloadTree {
            conflict: Conflict::Overwrite,
            ..
        }
    ));
    app.lock_vault();
    assert!(app.prepare_transfer(&request).is_err());
}

#[test]
fn agent_transfer_approval_and_stop_hold_and_cancel_file_writes() {
    let mut app = NativeApp::new(Some(fixture_vault("scp-approval", true)));
    app.settings.automation = true;
    app.settings.approve_commands = true;
    let mut request = request();
    request.host = app.vault.as_ref().unwrap().hosts()[0].id;
    let transfer = app.prepare_transfer(&request).unwrap();
    let tab = transfer.tab;
    let approval = transfer.approval.unwrap();
    app.transfers.push(transfer);
    app.move_transfers();
    assert!(
        app.tabs[app.active]
            .sftp
            .as_ref()
            .unwrap()
            .tab
            .worker
            .is_none()
    );
    assert!(
        app.tabs[app.active]
            .sftp
            .as_ref()
            .unwrap()
            .tab
            .transfers
            .is_empty()
    );
    assert!(app.pending_approval(tab).unwrap().command.contains("scp"));
    app.decide_approval(approval, false);
    assert!(app.transfers.is_empty());
    let transfer = app.prepare_transfer(&request).unwrap();
    let approval = transfer.approval.unwrap();
    app.transfers.push(transfer);
    app.decide_approval(approval, true);
    assert!(app.transfers[0].approval.is_none());
    // A connected fixture lets the real queue run without opening a network socket.
    let files = &mut app.tabs[app.active].sftp.as_mut().unwrap().tab;
    files.start = None;
    files.cwd = Some("/tmp".into());
    app.move_transfers();
    let cancel = app.tabs[app.active].sftp.as_ref().unwrap().tab.transfers[0]
        .cancel
        .clone();
    assert!(!cancel.load(Ordering::Relaxed));
    app.cut_runs(Instant::now());
    assert!(cancel.load(Ordering::Relaxed));
    assert!(app.transfers.is_empty());
}

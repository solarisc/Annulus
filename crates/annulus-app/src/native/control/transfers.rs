//! Agent file copies use visible SFTP tabs, saved logins and the normal transfer worker.

use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::Instant;

use annulus_control::door::Pending;
use annulus_control::protocol::{Response, TRANSFER_CLEANUP, TransferOutput, TransferRequest};
use annulus_sftp::Conflict;

use super::{AgentCommand, AgentLog, NativeApp, PendingApproval, Ran, Typed};
use crate::sftp_view::{Recipe, TransferState};

/// A call waiting for approval, a connection, or a completed copy.
pub struct TransferCall {
    call: Option<Pending>,
    tab: u64,
    recipe: Recipe,
    label: String,
    approval: Option<u64>,
    transfer: Option<u64>,
    since: Instant,
}

impl TransferCall {
    fn answer(mut self, result: Result<TransferOutput, String>) {
        if let Some(call) = self.call.take() {
            call.answer(
                result
                    .map(Response::Transferred)
                    .map_err(|why| format!("{why}\n{TRANSFER_CLEANUP}")),
            );
        }
    }
}

/// Checks exact destinations before opening a tab or touching files.
fn recipe(request: &TransferRequest) -> Result<Recipe, String> {
    let local = Path::new(&request.local);
    if !local.is_absolute()
        || request.remote.is_empty()
        || request.local.chars().any(char::is_control)
        || request.remote.chars().any(char::is_control)
        || request.remote.contains('\\')
    {
        return Err("scp needs an absolute local path and a remote path with forward slashes and no control characters".into());
    }
    let conflict = if request.overwrite {
        Conflict::Overwrite
    } else {
        Conflict::Refuse
    };
    let remote = request.remote.trim_end_matches('/');
    let (dir, name) = remote
        .rsplit_once('/')
        .map_or((".", remote), |(dir, name)| {
            (if dir.is_empty() { "/" } else { dir }, name)
        });
    if matches!(name, "" | "." | "..") {
        return Err("the remote path must name a file or directory, not a root".into());
    }
    if request.upload {
        if local.file_name().is_none() {
            return Err("the local source must name a file or directory".into());
        }
        let (local, dir, name) = (local.to_path_buf(), dir.to_owned(), Some(name.to_owned()));
        Ok(if request.recursive {
            Recipe::UploadTree {
                local,
                dir,
                name,
                conflict,
            }
        } else {
            Recipe::Upload {
                local,
                dir,
                name,
                conflict,
            }
        })
    } else {
        let name = local
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| annulus_sftp::is_safe_local_name(name))
            .ok_or("the local destination needs a safe file or directory name")?;
        let to = local
            .parent()
            .ok_or("the local destination needs a parent folder")?
            .to_path_buf();
        let (remote, name) = (request.remote.clone(), Some(name.to_owned()));
        Ok(if request.recursive {
            Recipe::DownloadTree {
                remote,
                to,
                name,
                conflict,
            }
        } else {
            Recipe::Download {
                remote,
                to,
                name,
                conflict,
            }
        })
    }
}

impl NativeApp {
    /// A held transfer over fixture files for UI checks, with no network connection.
    #[cfg(test)]
    pub(crate) fn preview_transfer(&mut self) {
        self.settings.automation = true;
        self.settings.approve_commands = true;
        let host = self.vault.as_ref().unwrap().hosts()[0].id;
        let request = TransferRequest {
            host,
            upload: true,
            local: std::env::temp_dir()
                .join("release.zip")
                .display()
                .to_string(),
            remote: "/srv/releases/release.zip".into(),
            recursive: false,
            overwrite: false,
        };
        let transfer = self.prepare_transfer(&request).unwrap();
        self.tabs[self.active].sftp = Some(Box::new(crate::native::sftp::SftpView::new(
            crate::native::demo::sftp_tab("Agent files"),
        )));
        self.transfers.push(transfer);
    }

    pub(super) fn queue_transfer(&mut self, call: Pending, request: &TransferRequest) {
        match self.prepare_transfer(request) {
            Ok(mut transfer) => {
                transfer.call = Some(call);
                self.transfers.push(transfer);
            }
            Err(why) => call.answer(Err(format!("{why}\n{TRANSFER_CLEANUP}"))),
        }
    }

    fn prepare_transfer(&mut self, request: &TransferRequest) -> Result<TransferCall, String> {
        if !self.settings.automation || self.vault.is_none() || self.blocked.is_some() {
            return Err("agent access is off or the vault is locked".into());
        }
        let recipe = recipe(request)?;
        let tab = self
            .sftp_for(request.host)
            .ok_or_else(|| format!("no saved host {}: see `annulus-agent hosts`", request.host))?;
        let label = format!(
            "scp{}{} {} {}",
            if request.recursive { " -r" } else { "" },
            if request.overwrite {
                " --overwrite"
            } else {
                ""
            },
            if request.upload {
                format!("{:?}", request.local)
            } else {
                format!("{}:{:?}", request.host, request.remote)
            },
            if request.upload {
                format!("{}:{:?}", request.host, request.remote)
            } else {
                format!("{:?}", request.local)
            }
        );
        self.push_sftp(tab);
        let tab = &mut self.tabs[self.active];
        tab.agent = Some(Box::new(AgentLog::new(crate::sync_screen::now_secs())));
        let approval = self.settings.approve_commands.then(|| {
            let id = self.next_approval;
            self.next_approval = self.next_approval.wrapping_add(1).max(1);
            id
        });
        Ok(TransferCall {
            call: None,
            tab: tab.uid,
            recipe,
            label,
            approval,
            transfer: None,
            since: Instant::now(),
        })
    }

    pub(super) fn transfer_approval(&self, tab: u64) -> Option<PendingApproval> {
        self.transfers.iter().find_map(|transfer| {
            (transfer.tab == tab).then_some(())?;
            Some(PendingApproval {
                id: transfer.approval?,
                command: transfer.label.clone(),
            })
        })
    }

    pub(super) fn decide_transfer(&mut self, id: u64, approved: bool) {
        let Some(index) = self.transfers.iter().position(|t| t.approval == Some(id)) else {
            return;
        };
        if approved {
            self.transfers[index].approval = None;
        } else {
            self.transfers
                .remove(index)
                .answer(Err("the owner denied this transfer".into()));
        }
    }

    pub(super) fn move_transfers(&mut self) {
        for mut transfer in std::mem::take(&mut self.transfers) {
            if !self.settings.approve_commands {
                transfer.approval = None;
            }
            if transfer.approval.is_some() {
                if self.tabs.iter().any(|tab| tab.uid == transfer.tab) {
                    self.transfers.push(transfer);
                } else {
                    transfer.answer(Err("the transfer tab closed".into()));
                }
                continue;
            }
            let result = self.advance_transfer(&mut transfer);
            match result {
                None => self.transfers.push(transfer),
                Some(result) => {
                    if let Some(log) = self
                        .tabs
                        .iter_mut()
                        .find(|t| t.uid == transfer.tab)
                        .and_then(|t| t.agent.as_mut())
                        && let Some(command) = log.commands.last_mut()
                    {
                        command.state = if result.is_ok() {
                            Ran::Done {
                                took: transfer.since.elapsed(),
                                lines: 0,
                            }
                        } else {
                            Ran::Ended {
                                took: transfer.since.elapsed(),
                            }
                        };
                    }
                    transfer.answer(result);
                }
            }
        }
    }

    fn advance_transfer(
        &mut self,
        transfer: &mut TransferCall,
    ) -> Option<Result<TransferOutput, String>> {
        let Some(tab) = self.tabs.iter_mut().find(|t| t.uid == transfer.tab) else {
            return Some(Err("the transfer tab closed".into()));
        };
        let view = tab.sftp.as_mut()?;
        let files = &mut view.tab;
        let wake = self.waker.clone();
        files.start_with(move || {
            if let Some(wake) = &wake {
                wake();
            }
        });
        files.drain();
        if let Some(why) = &files.failed {
            return Some(Err(why.clone()));
        }
        files.cwd.as_ref()?;
        if transfer.transfer.is_none() {
            files.start_transfer(transfer.label.clone(), transfer.recipe.clone());
            transfer.transfer = Some(files.next_id);
            transfer.since = Instant::now();
            if let Some(log) = &mut tab.agent {
                log.commands.push(AgentCommand {
                    text: transfer.label.clone(),
                    at: crate::sync_screen::now_secs(),
                    line: 0,
                    state: Ran::Running(transfer.since),
                    typed: Typed::default(),
                });
            }
        }
        let copied = files
            .transfers
            .iter()
            .find(|t| Some(t.id) == transfer.transfer)?;
        match &copied.state {
            TransferState::Running => None,
            TransferState::Done(to) => Some(Ok(TransferOutput {
                tab: transfer.tab,
                to: to.clone(),
                bytes: copied.done,
                cleanup: TRANSFER_CLEANUP.into(),
            })),
            TransferState::Skipped => Some(Err(
                "the destination exists; use --overwrite only if you intend to replace it".into(),
            )),
            TransferState::Failed(why) => Some(Err(why.clone())),
        }
    }

    pub(super) fn cut_transfers(&mut self) {
        for transfer in std::mem::take(&mut self.transfers) {
            if let Some(tab) = self.tabs.iter_mut().find(|t| t.uid == transfer.tab) {
                if let Some(view) = &mut tab.sftp {
                    for copied in &view.tab.transfers {
                        if Some(copied.id) == transfer.transfer {
                            copied.cancel.store(true, Ordering::Relaxed);
                        }
                    }
                }
                if let Some(log) = &mut tab.agent
                    && let Some(command) = log.commands.last_mut()
                {
                    command.state = Ran::Cut {
                        took: transfer.since.elapsed(),
                    };
                }
            }
            transfer.answer(Err("agent access ended; the transfer was cancelled".into()));
        }
    }
}

#[cfg(test)]
mod tests;

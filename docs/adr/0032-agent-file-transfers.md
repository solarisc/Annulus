# ADR-0032: Agent file transfers through saved hosts

- Status: Accepted (2026-11-09)
- Date: 2026-11-09
- Extends: [ADR-0020](0020-local-control-channel-for-cli-and-ai.md) and [ADR-0031](0031-agent-settings-approval-and-skill.md)

## Context

Agents can run commands through Annulus, but cannot copy local files through a saved SSH
login. The maintainer wants `annulus-agent.exe` to offer SCP file transfers and tell agents to
clean up their temporary files safely.

## Decision

1. `annulus-agent scp [-r] [--overwrite] <source> <destination>` copies in either direction.
   Exactly one path names a saved host as `<host-id>:<path>`. A trailing `/` means a target
   folder; otherwise the destination names the exact file or directory. MCP exposes the same
   call as `annulus_scp`. `--json` returns the tab id, target, bytes copied and cleanup advice.
2. The app uses its existing SFTP worker, saved login, jump hosts and SSH checks. The command
   does not launch a shell or export credentials. It requires the host's SFTP subsystem;
   the legacy SCP wire protocol is outside this change. Remote paths use forward slashes.
   No shell expands wildcards or `~`. Directory copies skip links.
3. Each call opens a visible AI file tab. The normal *Approve commands* setting also holds
   file copies until the user approves their exact paths and flags. Deny fails the call.
   Stop, lock and turning agent access off cancel queued and running copies. The tab keeps
   the transfer result, and its Cancel button can stop a running copy.
4. Copies refuse an existing target unless the caller passes `--overwrite`. With that flag,
   directory copies merge their contents and replace files of the same name. A completed
   copy returns its real destination and byte count. A failure returns an error, not a
   success message. A failed directory copy may leave files it finished before the error.
5. Text, JSON and MCP results ask agents to remove only temporary files they created, after
   checking the copy and the exact paths, once the task no longer needs them. They must keep
   sources, final outputs and other people's files. The bundled skill gives the same advice.
   The app does not delete source files. Existing SFTP rules remove owned part files on
   normal cancellation and keep them after a lost connection for retry. Abrupt closure can
   leave a part file; agents must prove ownership and check that no copy uses it before
   removing it.

## Consequences

- Agents can move binary files without encoding them into terminal commands.
- File copies share the app's login and transfer checks, and remain visible to the user.
- Copies can wait for host-key or login questions in the file tab. They run off the UI thread.
- A local path names a path on the computer running Annulus. Remote-to-remote copies require
  two calls with a local file between them.

## Alternatives considered

- **Run the system `scp`.** It would need access to vault credentials or a separate login.
- **Build a second transfer worker.** The app already has SFTP, progress, cancellation and
  part-file handling.
- **Delete temporary files automatically.** The app cannot know which task still needs them.

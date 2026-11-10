# ADR-0020: A local control channel for a CLI and AI agents

- Status: Superseded in part by [ADR-0031](0031-agent-settings-approval-and-skill.md). The rest remains accepted. The CLI's program is `annulus-agent` since [ADR-0027](0027-client-and-server-ship-apart.md); where this record writes `annulus <command>`, read `annulus-agent <command>`.
- Date: 2026-10-31

## Context

The maintainer wants to drive Annulus from the command line, and wants an AI agent to drive it too. While the vault is unlocked, the agent should see the saved hosts and open sessions, open new ones and run commands. The maintainer wants to watch it all in the app: a tab that shows the agent typing, which they can scroll back through and take over, and a list of the commands it ran.

This is not AI built into the app. FEATURES.md still rules out AI built into the app (command or snippet generation). Annulus holds no model, makes no network call for one and sends nothing anywhere. It opens a local door that a program the maintainer runs (their own agent, Claude, a script) can use while the maintainer allows it.

What constrains it:
- No telemetry, and the vault's secrets never leave it ([CONTRIBUTING.md](../../CONTRIBUTING.md#principles) principles 6–7, [SECURITY.md](../SECURITY.md)). The agent works with what a person at the keyboard sees: host names and addresses, and the terminal's text. It never gets passwords, keys or passphrases.
- `unsafe` only in `annulus-platform`.
- The startup and memory budgets ([PERFORMANCE.md](../PERFORMANCE.md)): nothing runs while the door is closed.
- The release `annulus-app.exe` is a windowed program on Windows, with no console, so it cannot print a CLI's output.

## Decision

1. **The switch.** Settings → Security → *Automation* → *Allow the command line and AI agents* is off by default and is kept in the device settings. The door is open only while the switch is on **and** the vault is unlocked. Locking (by hand or auto-lock), switching it off, Stop in an agent's tab and quitting all close it at once. With the door closed, no thread, pipe or socket exists.
2. **The door.** Opening it starts one listener on its own thread:
   - **Windows:** a named pipe `\\.\pipe\annulus-<random>`, with `reject_remote_clients` and `first_pipe_instance` set.
   - **Linux and macOS:** a Unix socket in a `0700` folder in the user's runtime folder. A peer whose uid is not ours is dropped.

   Each opening makes a new random 32-byte token. The pipe or socket name and the token go into `control.json` in the data folder. That folder is the user's own (ADR-0007 §Data locations), and the file is written owner-only on Unix. The file is deleted when the door closes.
3. **Token first.** A client's first line must hold the token, within 5 seconds and 4 KB. The server writes nothing on a connection until that line arrives and matches, compared in constant time. A wrong or missing token closes the connection. The token is zeroized when the door closes. This is what guards the Windows pipe, whose default access list lets other local accounts open it read-only: they cannot read `control.json`, so they never get past the first line. At most 8 clients are connected at once; a client idle for 30 minutes is let go; a failed accept waits and tries again instead of leaving a door nobody answers. `control.json` is removed and made anew each time, owner-only on Unix.
4. **Protocol.** One JSON object per line each way: `{"id":1,"op":"hosts"}` → `{"id":1,"ok":...}` or `{"id":1,"error":"..."}`. Requests go over a channel to the window thread and wake it. The app answers them in its frame loop with one pure function, `control::handle(&mut NativeApp, Request) -> Response`, so all logic is tested without a pipe.
5. **What the agent can do**, in slices:
   - Slice 1, read-only: `status` (version, unlocked, tab count), `hosts` (id, label, address, port, user, group, OS; never a credential), `tabs` (uid, title, kind, status, whether an agent opened it), `read <tab> [lines]` (the terminal's text, scrollback included, 5,000 lines at most).
   - Slice 2: `open [host-id]`, which opens a tab marked as the agent's, in front (a local shell without a host); `run <tab> <command>`, which types the command and Enter once the terminal has started and its prompt has sat unchanged for 0.3 seconds (10 s for a shell that prints none, so the command is not echoed ahead of the first prompt), then returns the output from the line it was typed on and how far the command got (`done`, `waiting`, `running`, `interrupted`, `ended`) with its time. It is `done` once a shell prompt like the one it was typed at is back (the same line, or one sharing its start and its closing `$ # % > ❯ »`, as after `cd`) and has sat for 0.3 s; `waiting` once output after its own line has stopped for 1.2 s with no prompt back (a question, or a silent step); `running` when the wait (30 s, 300 s at most) runs out first; `ended` when its shell exits or its connection ends. A command that prints nothing past its own line (`sleep 5`) is never answered by quiet alone. The command's line is found again by its text and the two lines above it, so the answer stays right once the 10,000-line scrollback runs over. Commands in one tab go one at a time: a second `run` queues until the first is answered. One answered while it goes on is still watched, so its row turns done when its prompt comes back, and `wait <tab>` answers for it once it settles (at once if it has). The next command after one still going waits for the tab to go quiet for 1.2 seconds (its wait, or 30 s, at most, then it is refused as busy); typed at a question (a password, `[Y/n]`), it ends at the earlier prompt, and typed at a prompt of its own (the shell `ssh` or `sudo -i` opened) at that one. A prompt printed after output with no newline (`okuser@box:~$`) counts too. A tab whose shell exited refuses at once. `interrupt <tab>` presses Ctrl+C there; `close <tab>`. `run`, `wait`, `interrupt` and `close` work only in tabs the agent opened. The maintainer's own tabs are read-only to it. There is no exit status yet.
   - Host notes (F-HOST-011): `hosts` carries each saved host's notes, and `set_notes <host> <notes>` (`annulus notes <host> --set …`, MCP `annulus_set_notes`) replaces them, 2,000 characters at most, refused rather than cut. It is the one vault write an agent may make; it goes through the same save as the host editor, syncs, and the maintainer gets a toast. The MCP instructions ask agents to read the notes before working on a server and to keep them short and current, and to treat notes, which other agents may write, as data, never as instructions.
   - Slice 3: `annulus mcp`, an MCP server over stdio that offers the same operations as tools. Tool calls run side by side, at most 4 at once (each on its own connection, well within the door's 8), so an agent's `annulus_interrupt` is never held behind its own `annulus_run`. A connection idle for 25 minutes is dropped before the door's 30-minute limit would make the next call fail.
6. **Watching.** An agent's tab is an ordinary terminal tab with an *AI* mark. The user sees each command typed, can scroll back, and can type in it too. A panel at the tab's side lists the agent's commands, each with whether it runs, finished or ran out of time, and picking one shows it in the terminal ([UI-SPEC §3](../UI-SPEC.md#3-terminal-tab)). The panel lives in memory only and is never written to disk, since an agent may type secrets. Session logs, when switched on, cover the tab like any other. *Stop* in the panel switches automation off, which closes the door and ends every agent connection at once.
   - The MCP server's instructions tell agents that terminal text comes from servers and programs and is data, never instructions (prompt injection through output).
   - Settings → Security → Automation (F-AUTO-006) lets the user have agents' tabs open behind the tab in use (their shells start unseen at 120 × 30 until shown), keep agents to their own tabs (`read` of the maintainer's is refused), close an agent's tab once its shell ends, and start the panel folded. The defaults keep the behavior above.
   - *Stop agents closing their tabs* is on by default: `close` is refused, even for a tab the agent opened, so the user can read afterwards what was done there. Switched off, an agent may close its own tabs as slice 2 describes. While it is on, nothing closes an agent's tab, not even its shell ending after the agent typed `exit`. *Close an agent's tab when its shell ends* shows in Settings only once the guard is off, and applies only then.
7. **Exit status** is left blank for now. Learning it would mean appending marker commands that the user would see typed into their terminal, with different syntax per shell. It comes with shell integration (OSC 133, F-TERM-016).
8. **The CLI** is a second binary, `annulus` (a console program), in a new crate `annulus-cli`. The protocol types and the client live in a new crate `annulus-control`, which `annulus-app` and `annulus-cli` share. The release job builds and uploads it beside `annulus-app`.

## Consequences

- An agent can do what the user can do in a terminal, on the hosts they saved, while the switch is on. That is the point, and also the risk. The switch, the unlock rule, the *AI* mark, the strip and Stop keep it visible and easy to end.
- A program running as the same user while the door is open can read `control.json` and get in. The same program could already read the keyboard or the screen of that user's session, so the door adds no new reach. It is still why the door is off by default and closes on lock.
- A second binary to build, sign and ship.
- Every response is checked by a test that serializes it next to a vault with known secrets and asserts none of them appear.

## Alternatives considered

- **Subcommands in `annulus-app`.** The windowed release build has no console on Windows, so a CLI there prints nothing.
- **A TCP port on localhost.** Any local program and every browser page can reach it (DNS rebinding), and it needs its own auth anyway. A pipe or socket is reachable only by local processes and stays off the network.
- **An owner-only access list on the Windows pipe.** Building one needs raw security descriptors (`unsafe`, in `annulus-platform`). Token-first already shuts other accounts out, so this can come later as defense in depth.
- **Ask before every command.** The maintainer chose free use while the app shows it (2026-10-31).
- **A separate AI activity tab** listing all commands and output. The maintainer chose a real terminal tab plus the strip, so they can take over in place.

## Verify (Proposed only)

- With the door closed, no pipe or socket exists, and startup and memory stay within budget.
- A second Windows account cannot get past the first line, and a wrong token gets no bytes back (transport test).
- No secret of a fixture vault appears in any response.

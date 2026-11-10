# UI spec

Last verified: 2026-11-09 @ 51a7d79

**Goal:** anyone who has used an SSH client finds their way within a minute: clear places, short flows, standard shortcuts. The look is **ours** ([ADR-0010](adr/0010-original-work-only.md)): our name, logo, icons, palette and copy. Only features in [FEATURES.md](FEATURES.md) appear here.

## 1. Window frame

```
┌──────────────────────────────────────────────────────────────────────────────────────────┐
│ ● ● ●  [≡] [Vault] [SFTP] │ [● web-1 ×] [● db-2 ×] [⌗ prod (3) ×] [+]    ⟳ Synced  _ □ × │  ← 40 px tab strip = title bar
├──────────────────────────────────────────────────────────────────────────────────────────┤
│                               active tab content                                         │
└──────────────────────────────────────────────────────────────────────────────────────────┘
```

- **The tab strip is the title bar.** Empty space drags the window, and a double-click maximizes it.
  - Windows and Linux: the window controls sit on the right. Windows keeps snap layouts.
  - macOS: native traffic lights sit on the left, centered in the strip, which keeps their room free. The app menu lives in the system menu bar: About Annulus (Settings → About), Settings… and Lock Vault, with their shortcuts' keys beside them, then macOS's own Services, Hide and Quit.
- **Fixed tabs:** `Vault` and `SFTP`, never closed or moved, then a hairline, the session and workspace tabs, then `+` (New Tab screen).
  - **SFTP** picks a saved host (search, recent connections first) and shows its files in place, beside this computer's; "Hosts" goes back to the picker.
  - **New Tab** (`+`, Ctrl+Shift+T): a search over the saved hosts that has the keyboard at once (Enter connects to the highlighted row, Up/Down move it, a typed `user@host` connects once); buttons for a local terminal, SFTP and a new host; then the hosts pinned to the top, in the order pinned, then this device's recent connections (at most six, with when) and the newest saved hosts filling the rest of the page. A host row's pin (shown on hover or the highlighted row, always on a pinned one) pins or unpins it; the page stays. What is chosen opens in the New Tab's place. Recent connections and pins are kept on this device by host id only (the settings file is not encrypted). While the vault has no host, the page welcomes instead: Import ssh_config (Hosts with the import preview open) and New host.
- **Session tab:** an icon (the server's OS mark for a saved host whose OS is known, else a terminal or folder; a turning arc while its session connects), the label (host label by default, renamable), and a close button on hover and on the tab in front. Only the tab in front and the one under the pointer show a shape.
  - The close button, middle click, Close command and close shortcut stop every live pane in the tab before removing it. An SSH connection closes even while it is still connecting or waiting to reconnect.
  - Drag a session tab to move it: it follows the pointer and its neighbours glide out of the way; Ctrl+Shift+PageUp/PageDown move the tab in front. Tabs grow in when opened and glide into place when another closes.
  - Status dots, on the icon's corner: accent = unread output, amber = input needed, red = error or disconnected.
  - Many tabs share the strip evenly, down to 96 px each. A tab narrower than it wants trims its padding and keeps room for its close button only while that shows, so its name keeps a few letters. The tabs that still don't fit wait in an **All tabs** menu (`⋯`, after `+`): every tab, the one in front checked, with a search once it holds more than twelve. The `⋯` carries the most urgent status dot of the tabs off the strip. The tab in front is always on the strip, in its last place: a tab picked from the menu or newly opened moves there.
  - Workspace tabs show a grid icon and their pane count. A grey dot marks unsaved workspace changes.
- **Sync mark:** one small cloud inside the Vault tab, only when the vault syncs to an account (nothing in local mode). A dot on it breathes while changes wait to go up and turns red when the last sync failed; the tooltip says which and when it last synced. A click opens Settings → Account & sync. There is no second sync indicator in the navigation.
- **`≡`** (Windows/Linux) opens the app menu: New Tab, Settings, Lock, About, Quit.

## 2. Vault tab

```
┌────────────────┬─────────────────────────────────────────────────────────────┬──────────────────────┐
│ ▣ Hosts        │ [🔍 Search hosts or type user@host           ] [Connect]     │  HOST EDITOR (360px) │
│ ⚿ Keychain     │ [+ New Host ▾] [>_ Terminal ▾]    [Tags ▾] [▦/☰] [Sort ▾]    │  Label   [web-1    ] │
│ ⇄ Port Fwd     │                                                             │  Address [10.0.0.4 ] │
│ { } Snippets   │ GROUPS                                                      │  Port    [22] Group ▾│
│ ✓ Known Hosts  │ [▢ Production · 12]  [▢ Staging · 4]  [▢ Homelab · 7]        │  Tags    [web] [+]   │
│ ☰ Logs         │                                                             │  ── Credentials ──   │
│                │ HOSTS                                                       │  User [root  ] [👤▾] │
│                │ [▙ web-1          ] [▙ db-2           ] [▙ cache-1        ]   │  ○ Password ● Key ▾  │
│                │ [  root@10.0.0.4  ] [  pg@10.0.0.9    ] [  ubuntu@cache   ]   │  [Show more ▾]       │
│ ─────────      │ [  web prod       ] [  db             ] [                 ]   │  inherited: from     │
│ ⚙ Settings     │                                                             │  "Production" ⓘ      │
└────────────────┴─────────────────────────────────────────────────────────────┴──────────────────────┘
   200px nav                       fluid content                                  side panel (closable)
```

- **Left nav order:** Hosts, Keychain, Port Forwarding, Snippets, Known Hosts, Logs, then Settings pinned at the bottom. The nav collapses to icons below 900 px of window width.
- **Hosts screen:**
  - The search field fuzzy-filters hosts and also accepts `user@host:port`. Enter connects to the top result, or quick-connects to a typed target.
  - The title, with the number of hosts listed set quietly beside it, shares a row with the actions (only Terminal while the vault is empty, whose own page offers New host and Import), above the search ([ADR-0021](adr/0021-quiet-look-and-choreographed-motion.md)): the grid/list toggle, Tags, Terminal, then New host at the right end. When the row can't fit the labels (beside the editor), the buttons show icons only and keep their names as tooltips. A heading over the hosts shows only when groups are listed above them or a search found them.
  - `+ New Host ▾` is one accent-filled split button. The left half adds a host; the chevron half, behind a thin divider, opens a menu under the whole button with New Group, Import (ssh_config, PuTTY) and Export ssh_config. Each half lights up and takes focus on its own. Terminal and Tags are ghost buttons, so the accent stays on the one main action.
  - `Terminal ▾` opens a local shell (default profile), or a chosen profile from the menu.
  - The group section shows cards with name and host count. Double-click a group to open it, which shows a breadcrumb.
  - Host cards show the OS tile (a wash of the OS's hue with its mark: the server's OS from its identification line, or one picked in the editor; initials when unknown), the label, `user@address` and the tags as colored chips on the address line (a "+n" chip for the rest; the address keeps at least half the line). They have only a faint edge at rest. Hover lifts the card and fades in one Edit button in room kept for it, so the text never moves or gets cut for it. The selected card has a neutral fill and a thin accent ring.
  - Double-click connects. Single-click selects; with **Connect with one click** on (Settings → Sessions, off by default) a click connects, as a row on the New Tab page always does, and the double-click it may become opens nothing more. `Enter` connects the selection. Right-click or Shift+F10 opens the context menu (Connect, Open SFTP, Edit, Copy ssh command, Duplicate, Move to …). Cards fill in one after another when the screen appears.
  - Grid/list toggle and sort control. In list view the columns are label, address, user, tags and last used. Drag a host in either view to change its saved place within its group. The card follows the pointer, a line marks the drop place and the other cards glide to their new places after the drop.
  - Context menus:
    - Host: Connect, Open SFTP, Edit, Copy ssh command, Duplicate, Move to group.
    - Group: Connect all, Edit, New subgroup, Delete (asks whether to keep or move its hosts).
- **Host editor (340 px side panel, Save and Cancel, `Esc` closes):** it travels its whole width in from the sheet's edge and back out with its form on it, and the list's top row and search follow its edge. Cards that stay in the same columns glide to their new places; when the columns change, the old grid fades out first and the new one fills in.
  - The sections are General, Credentials, and **Show more**. Show more holds jump hosts, proxy, startup snippet, environment, agent forwarding, keepalive, auto-reconnect, session logging, terminal theme/font, algorithms, and auth order.
  - An inherited field shows its value greyed with "from <group>", and typing overrides it. The "↺" button returns the field to inheriting.
  - Validation errors appear inline. A host can't be saved without an address.
  - A saved host's editor has **Duplicate** beside Close: a copy of what the form shows is saved as "… copy" and opens in the editor, while the host keeps what was saved (the toast says so when the form had unsaved changes).
  - **Notes** (F-HOST-011) sit under Icon: a box of about five lines for what to know about the server, 2,000 characters at most; long lines wrap at its width. Agents read and may rewrite them, and a toast says when one did; notes an agent changed while the editor was open stay unless the box was edited too.
- **Keychain:** tabs for Keys, Identities and Certificates.
  - `+ ▾` offers Generate key, Import key, New identity, FIDO2 key, and Device key (Hello/Touch ID; hidden on Linux).
  - Key detail shows type, fingerprint, comment, a copy-public-key button and Export to host.
  - A click on a key (or its Edit button) opens it in a card at the top: its name, its private key as dots until the eye shows it (an explicit choice each time; the card forgets it when it folds away), **Copy private key** (a secret copy: kept from clipboard history and cleared after 30 s), the public key, and Save. Pasting another key over it replaces it on Save under the same id, so its hosts keep it, and the line says its public key changed. A field showing a key still never copies it out with Ctrl+C, and screen readers are not handed its text (`widgets::Revealed`).
  - Delete on a key no host uses asks once more in its row. On a key hosts log in with (their own or their group's), it opens a card naming them, with "Also take it off" ticked: the key comes off each server's `authorized_keys` first, then leaves the vault, and a server that could not remove it keeps the key here with the reason.
- **Port Forwarding:** a list of rules with a status toggle on each row, then Edit, Duplicate and Delete. Errors appear inline on the row; a rule that listens beyond this computer (bound to `0.0.0.0` or another non-loopback address) shows a warning mark and "Open to your network" on its row, also in its accessible name; Edit opens a form with type tabs (Local, Remote, Dynamic), and Duplicate opens the same form as a new rule named "… copy" with its port left to choose.
  - `+ New rule` opens a wizard (F-FWD-006), which is also the whole screen while there are no rules. Step 1 shows a card per kind, each with a diagram: this computer, the server and the far side, an encrypted tunnel between the first two, arrows the way traffic goes, and a packet that crosses once when the diagram shows and again on hover (never on a loop, so an idle window asks for no frames; none under reduced motion). Step 2 picks the server and ports while a large diagram shows the values typed. Step 3 says the rule in one sentence, takes a name and the start options, and offers Save or Save and start.
- **Snippets:** packages on the left and snippets on the right. The editor holds the label and a script area (monospace). Actions: Run on hosts…, Copy, Delete.
- **Known Hosts:** a table of host:port, key type, fingerprint and date added. Actions: delete, convert to host.
- **Logs:** a list of host, start time, duration, size and bookmark. Opening one shows a read-only terminal tab with search. A row's Delete button, or its context menu (right click or Shift+F10: Open, Export to Downloads, Delete), removes a log after a yes in a confirmation card.

## 3. Terminal tab

```
┌──────────────────────────────────────────────────────────────┬───────────────┐
│ root@web-1:~$ systemctl status nginx                         │ SIDE PANEL    │ (Ctrl+Shift+. / Cmd+.)
│ ● nginx.service - A high performance web server              │ [Snippets]    │
│    Active: active (running)                                  │ [History]     │
│ root@web-1:~$ █                                              │ [Theme/Font]  │
│                            ┌────────────────────────┐        │               │
│                            │ systemctl restart nginx│ ← autocomplete popup    │
│                            │ systemctl status nginx │        │               │
│                            └────────────────────────┘        │               │
├──────────────────────────────────────────────────────────────┴───────────────┤
│ 🔍 [find…        ] Aa .* ↑ ↓  3/12                                   [×]     │ ← find bar (on demand)
└──────────────────────────────────────────────────────────────────────────────┘
```

- **Split view:** panes have a 1 px divider and a 4 px drag handle. The focused pane gets an accent outline. A new split slides and fades in from its edge while the old pane and divider glide to their new bounds; its control bar slides down as one piece. Terminal text redraws at its true size and never scales. Reduce motion puts the split at its end at once. Broadcast shows an amber banner ("Typing into 4 panes") plus amber outlines on every receiving pane.
- **Focus mode:** a large pane, plus a 220 px list of the other sessions on the right.
- **Disconnect:** an inline banner offers Reconnect or Close, and auto-reconnect shows a countdown. Output stays.
- **Context menu:** Copy, Paste, Select all, Find, Clear, Split right/down, Open SFTP here, Snippets ▸, Reset terminal.
- **Scroll bar** (F-TERM-017): a 5 px thumb down the right margin, as tall as the screen is of the scrollback (24 px at least), faint until hovered. Dragging it scrolls; a press on the track takes the thumb by its middle there. An accent tick marks each row an agent typed on. Nothing shows while everything fits, or in a full-screen program (vim, less). Scrollback keeps 10,000 lines unless Settings → Terminal says 1,000, 50,000 or 100,000 (F-TERM-004).
- **Side panels** (the agent panel, the snippet panel): rounded cards of their own beside the terminal's card, 8 px apart and clear of the window's edges like it.
- **Agent file copies** (F-AUTO-009, ADR-0032): `scp` opens an AI file tab with the usual two panes and transfer list. Its agent panel shows the copy's paths and flags, with Approve/Deny when required and Stop to cancel agent work. File-pane controls come before the right-hand panel in keyboard order.
- **Agent panel** (an agent's tab only, [ADR-0020](adr/0020-local-control-channel-for-cli-and-ai.md), F-AUTO-004): 320 px on the right.
  - Head: the *AI* chip, "Agent session", *Stop* (danger, a square stop mark, never a cross) and fold. Under it, on two lines so neither is cut short: a status dot (connected, waiting for an answer, dropped) with the tab's title, then when the agent opened it, how many commands it ran and how many are still running.
  - After Stop the head says "Agents stopped" with a grey chip and "Agents are cut off from Annulus." in amber, and *Stop* becomes *Allow again*, which switches Automation back on.
  - With Settings → Agents → *Approve commands* on, a new command comes first with its exact text, **Approve** and **Deny**. The panel unfolds and its tab comes to the front. No bytes reach the terminal before Approve; Deny sends the caller an error.
  - List: every command, newest last, kept in view as new ones come. Each row has a mark (a turning arc while it runs, a check once its prompt is back, an amber keyboard when its output stopped with no prompt back, a clock when its wait ran out, a cross when interrupted, cut off by Stop or its shell ended; a row answered while it went on turns to a check when its prompt comes back), the command in mono wrapped to 3 lines (the rest in its tooltip), and a line under it: when in the session (`+1:24`), how long it took (to its last output) and how many lines it printed. Hovering shows *Copy*.
  - Picking a row scrolls the terminal to the command and tints its rows up to the next command; picking it again goes back to the newest output.
  - The terminal's margin carries an accent bar beside each row an agent typed on.
  - Folded, the panel is a 44 px rail: unfold, the *AI* chip, then the count over the last command's mark (one button that opens the panel, its tooltip the session line from the head), and Stop (or Allow again) at the foot.
  - Foot: "Types only in this tab. Yours are read-only to it."

## 4. SFTP tab

```
┌──────────────── Local ▾ ─────────────────┬──────────────── web-1 ▾ ───────────────┐
│ C:\Users\me\projects        [↑][⟳][⋯]    │ /var/www/html            [↑][⟳][⋯]    │
│ Name             Size   Modified         │ Name             Size   Modified  Perm │
│ 📁 site          —      2026-10-20       │ 📁 assets        —      2026-10-21 755 │
│ 📄 index.html    4 KB   2026-10-27       │ 📄 index.html    4 KB   2026-10-22 644 │
├──────────────────────────────────────────┴────────────────────────────────────────┤
│ Transfers (2)  index.html → /var/www/html  ███████░░░ 70%  3.2 MB/s  ETA 2s  [×]   │
└────────────────────────────────────────────────────────────────────────────────────┘
```

- `⋯` menu: Show hidden files, New folder, Sort, Filter. Each pane has a filter of its own, so the two lists line up.
- **Columns** (both panes): Name, then Size, Type, Modified, Permissions and Owner by default; Group, Accessed and Created on asking. A right click on the headers shows or hides one; the choice is kept in the settings, per pane. Each header sorts (again: the other way round), folders always first. Permissions read `rwxr-xr-x` on a host or a Unix computer, and Read-only or Read/write (and hidden, system) on Windows; Owner and Group show names where the host lets anyone read `/etc/passwd` and `/etc/group`, else the ids. Windows keeps no Unix owner, so this computer's pane offers no Owner or Group there; a host says nothing of creation, so its pane offers no Created. A narrow pane cuts the columns last in its list first, and the one asked for last comes first, so it shows; the menu says which are cut for room. A hovered row's buttons come up over its last columns.
- Row context menu (right click, Shift+F10 or the row's More button). The host's rows: Open (folders), Download, Rename, Permissions…, Properties, Copy path, New folder, Refresh, Delete. This computer's rows: Open (a folder here, a file in its own app), Upload to the host, Show in Explorer (Finder, the file manager), Rename, Copy path, Properties, New folder, Refresh, Delete. F2 renames and Delete deletes the selection of the pane last clicked in; deleting asks first, in either pane. The empty space under the rows: New folder, Refresh, Show in Explorer (this computer), Copy path, Show or Hide hidden files. Not built yet: Open with… and editing a host's file in place (F-SFTP-006).
- **Drag and drop.** A row dragged onto the other pane uploads or downloads it; files dragged in from the OS upload to the host, or, let go over this computer's pane, are copied into its folder. While anything is dragged, every pane it may land in is outlined, and the one under the pointer fills with the accent and shows a badge low in it: what will happen (Upload to web-1, Download here, Copy here), to what, and into which folder. A label follows a dragged row. Where the OS does not say where its files are (macOS and Linux), only the host's pane takes them.
- **Taken names** (F-SFTP-005). The Existing files menu starts at *ask*: a file or folder whose name is already where it goes waits on a card saying what is there now and what is coming, with Skip, Replace (a folder: Merge, files of the same name inside replaced) and Rename, whose field starts with a free name (`report (1).txt`) and refuses a taken one. With more waiting, "Do the same for the N others waiting" answers them all (a rename gives each a free name). Keep both, Overwrite and Skip in the same menu answer without asking. A replaced file goes only once the whole new copy is there.

## 5. Dialogs and flows

- **First run:** a welcome screen with three choices: "Use locally" (create a master password), "Sign in to my server", or "Create account on my server". It then offers to import from ssh_config or PuTTY.
- **Create account:** server URL (checked live with a TLS and version check), username, master password (strength meter, minimum strength enforced), then the **recovery key** page with Copy, Save PDF and Print. The user must type the last 8 characters before continuing.
- **Lock screen:** the product's name at the top; then the lock, a title, the master password field, PIN or biometric buttons when set up, and a "Forgot password? Use recovery key" link, with no card around them and the navigation resting dim behind. While the key is derived, the field and recovery link dim and take no input, three points move around the lock with soft rings, clear status text replaces the detail and a band of light passes across the button. Reduced motion keeps a still ring and no sweep. A refused password shakes the field and the reason takes the place of the line under the title, so nothing moves up or down. On success the shackle lifts and a soft band opens from the center over 840 ms to show the working Hosts screen below. The lock text clears early, and the vault works from the first frame.
- **Unknown host key:** a modal titled "First connection to db-2" showing the key type and SHA256 fingerprint in monospace. Buttons: [Trust and connect] (primary), [Connect once], [Cancel].
- **Changed host key:** a red modal: "The host key for db-2 has changed." It shows the old and new fingerprints and explains that this may be an attack. The only buttons are [Cancel] and a secondary [Remove old key…]. That button opens a confirmation, after which the unknown-key flow runs.
- **Keyboard-interactive:** a modal showing the server's instructions and one field per prompt, with echo respected.
- **Connecting:** the host's tile flies from its card to its new tab's icon. Until the shell draws, the tab shows the host, `SSH user@host:port` and four steps (Server, Key, Login, Shell) on a track that fills as each passes; a passed step pops, the step under way spins, a step waiting on a question (a new host key, a password) says so, and Show log lists what happened (each jump host's steps too). The steps come from the SSH client (`annulus_ssh::Step`), not from guesses. They fade out over the first output.
- **Connection error:** an inline panel in the tab. It names the failing step and its cause, offers Retry, Edit host and Details, and can show the connection details.
- **Sync conflict:** a toast: "db-1 was changed on two devices. Kept the newer version." Its [Compare] button opens a side-by-side view with Restore.
- **Empty states:** each screen gets one sentence and a primary action, e.g. Hosts: "No hosts yet" with [New Host] and [Import].

## 6. Settings sections

Settings has its own navigation down the left of its panel (the selection glides between sections); the chosen section shows its title and one line on what it is for, then its cards, and rises into place when chosen. Only settings that exist are shown. Built today: Account & sync (with a vault file; short work and result lines change the card height in the same frame), Appearance (theme, color scheme, reduce motion), Terminal (colors, text size, a live preview in the chosen colors and size, and History: the lines each tab keeps, each choice with the memory a full tab costs), Security (auto-lock and a copy of the main agent access switch), Agents (the same access switch, command approval, tab behavior and the skill installer, ADR-0031), Sessions (reopen tabs, session logs, connect with one click), Local shells (default profile), Shortcuts (every shortcut from the one table the app checks keys against, `native::shortcuts`, grouped, searchable, as keycaps; click a shortcut's keys and press others to change them, see §7) and About. The table below is the full plan.

| Section | Contents |
|---|---|
| Account & Sync | Server, account, sync status, devices (rename/revoke), keep credentials device-local, change password, recovery key, export/import, sign out, remove local data |
| Security | Auto-lock timeout, lock on OS lock/sleep, PIN, Windows Hello/Touch ID, clipboard auto-clear |
| Agents | Local agent access, command approval, agent tab behavior, portable skill installer |
| Appearance | Theme (system/light/dark), UI scale, reduce motion |
| Terminal | Font, size, line height, ligatures, cursor style and blink, scrollback, bell, copy-on-select, right-click action, paste warnings, OSC 52 policy, default TERM, default color theme, Option-as-Meta, autocomplete settings |
| Local shells | Profiles, default profile, start directory |
| SSH | Default keepalive, auto-reconnect, PQ KEX, use OS agent, default auth order |
| Shortcuts | Keymap editor, preset (Default / Alternative), reset |
| SFTP | Default open-with per extension, conflict policy, show hidden files by default |
| Logs | Default recording policy, retention count |
| Advanced | Debug log, update check (opt-in), data folder location |
| About | Version, licenses, "not affiliated" notice |

## 7. Keyboard shortcuts

**Rule:** on Windows and Linux, `Ctrl+<letter>` belongs to the shell while a terminal has focus. App chords there use `Ctrl+Shift`. On macOS, `Cmd` is free. The optional **alternative preset** (`Ctrl+T`, `Ctrl+K`, `Ctrl+L`, …) exists for familiarity, and the keymap editor warns that it shadows readline keys.

| Action | Windows / Linux | macOS |
|---|---|---|
| Command palette | Ctrl+Shift+P | Cmd+Shift+P |
| New Tab screen | Ctrl+Shift+T | Cmd+T |
| Local terminal (default profile) | Ctrl+Shift+L | Cmd+L |
| Close tab/pane | Ctrl+Shift+W | Cmd+W |
| Next / previous tab | Ctrl+Tab / Ctrl+Shift+Tab | Cmd+Shift+] / [ |
| Move the tab left / right | Ctrl+Shift+PageUp / Ctrl+Shift+PageDown | Cmd+Option+Shift+[ / ] |
| Go to tab 1–9 | Alt+1…9 | Cmd+1…9 |
| Split right / down | Ctrl+Shift+D / Ctrl+Shift+E | Cmd+D / Cmd+Shift+D |
| Move focus between panes | Alt+Arrows | Cmd+Option+Arrows |
| Focus/Split mode | Ctrl+Shift+M | Cmd+Shift+M |
| Broadcast input | Ctrl+Shift+B | Cmd+Shift+B |
| Leave this pane out of broadcast (or back in) | Ctrl+Alt+Shift+B | Cmd+Option+Shift+B |
| Save workspace | Ctrl+Shift+S | Cmd+S |
| Side panel | Ctrl+Shift+. | Cmd+. |
| Find | Ctrl+Shift+F | Cmd+F |
| Links on screen (open one with the keyboard) | Ctrl+Shift+O | Cmd+Shift+O |
| Copy / Paste | Ctrl+Shift+C / Ctrl+Shift+V, Shift+Insert | Cmd+C / Cmd+V |
| Clear scrollback | Ctrl+Shift+Backspace | Cmd+Shift+K |
| Zoom in / out / reset | Ctrl+= / Ctrl+- / Ctrl+0 | Cmd+= / Cmd+- / Cmd+0 |
| Reconnect | Ctrl+Shift+R | Cmd+R |
| New host (Vault focused) | Ctrl+Shift+N | Cmd+N |
| Settings | Ctrl+, | Cmd+, |
| Lock app | Ctrl+Alt+L | Cmd+Ctrl+L |
| A host's or row's context menu | Shift+F10 (or right click) | Ctrl+click |
| Open a link in the terminal | Ctrl+click | Cmd+click |

**Command palette (F-APP-005).** The shortcut drops a small surface from the top edge over 560 ms, then spreads it into a centered search window over a blurred, inert view. Reduce motion shows the end state at once. The search lists saved hosts first, then open sessions, saved groups, keys, snippets and forwarding rules, every Vault and SFTP page, every Settings page, and app commands. Hidden terms include host addresses, tags and page topics. `Edit <name>` and `Edit (<name>)` open an item for changes; `SFTP <host>` opens that host in SFTP; `Connect`, `Open` and `Go` narrow the matching action. A typed `user@host[:port]` starts a quick connection. Up, Down, Home and End move the choice; Enter runs it; Escape or a click on the view behind it closes it and returns focus. The list shows at most eight matches. While open, only the dialog appears in the accessibility tree.

The native UI's chords live in one table (`native::shortcuts`) with both columns. The app answers to this computer's column, and Settings → Shortcuts lists it. A test fails when a listed chord is used twice on either keyboard or nothing checks for it. On a Mac the list names keys the Mac's way (Cmd, Ctrl, Option, Shift), a click with Ctrl held is a right click, and keys held with Cmd never reach the terminal's program. The menu bar's Settings… and Lock Vault show their shortcuts' keys and take them before the window sees them. Not built yet: Save workspace (on a Mac it will share Cmd+S with saving the host being edited), Links on screen and Reconnect in the native terminal.

**Changing keys (F-SET-004).** In Settings → Shortcuts, a click on a shortcut's keys (or Enter on them) records new ones: the field says "Press the new keys", Escape stops, and nothing else reacts to keys meanwhile. Keys are refused, with the reason under the shortcut and a shake of the field, in three cases. The first is keys that hold neither Ctrl, Alt nor the Windows key; on a Mac, keys that hold neither Cmd nor Ctrl, since Option with a key types a character there (F1–F24 alone are fine). The second is Ctrl with a letter and no Shift: those are the shell's, unless they are that shortcut's own, and the hint says to add Shift (on a Mac, to use Cmd). The third is keys another shortcut has, the listed ones without a chord included (Alt+1…9, Alt+Arrows, Shift+F10, Ctrl+Shift+V, Enter; on a Mac Cmd+1…9, Cmd+Option+Arrows, Cmd+V). While keys are recorded, the menu bar takes none. A changed shortcut shows Reset, refused with a toast when another shortcut has taken its own keys meanwhile; Reset all, beside the search, gives every shortcut its own keys again. Every other place that names a shortcut's keys (tooltips, the broadcast banner, Settings' explanations) asks the table, so it says the keys chosen. The choices are stored on this device in `settings.json` as the shortcut's name and its keys as written ("Ctrl+Alt+N", on a Mac "Cmd+Option+N"; either keyboard's names read back). The shortcuts listed without a chord cannot be changed. Not built yet: the alternative preset.

## 8. Design tokens

Every screen is photographed by CI in both themes ([ADR-0017](adr/0017-native-ui.md), the `scenes-*` artifacts); review them after any visual change. These are our own identity: cool charcoal and silver neutrals with the mark’s teal accent ([ADR-0021](adr/0021-quiet-look-and-choreographed-motion.md)). The accent is for the screen's main action, focus, links, progress and things that are on; selection is neutral. Hover, press, selection and text fields are drawn as veils of the text color, so they read on any surface; `bg.hover` and `bg.selected` are what they look like on a panel, the colors contrast is held to. The values live in `annulus-config::tokens`, and the test `both_themes_meet_wcag_aa` holds them to WCAG AA: 4.5:1 for text (and for accent links and error text on panels), 3:1 for the focus ring and status dots. A later pass darkened the light accent and lightened the dark error color to pass.

| Token | Dark | Light |
|---|---|---|
| `bg.app` | #0D0D0F | #F2F4F6 |
| `bg.surface` | #17181B | #FFFFFF |
| `bg.raised` | #27292D | #FFFFFF (with shadow) |
| `bg.hover` | #25272B | #F3F3F3 |
| `bg.selected` | #2D3034 | #EAEDF0 |
| `border.subtle` | #34363A | #DEE2E6 |
| `text.primary` | #DCDBDC | #1B1D21 |
| `text.secondary` | #ACAFB5 | #50555D |
| `accent` | #64E7D5 | #087569 |
| `status.warn` | #E0A43A | #B97B10 |
| `status.error` | #EF746D | #C93A33 |
| `status.ok` | #4CC27A | #24934F |
| `focus.ring` | accent, 2 px, offset 2 px, with a soft halo | same |

- **Host tiles:** eight hues of even lightness (`tokens::TILES`), one per host from its label, softened toward grey and drawn as a low wash with a clean bright mark or initials, at 4.5:1 or better (`tokens::tile_colors`).
- **Radii:** 4 (chips), 8 (buttons, fields, rows), 10 (cards), 12 (menus, the large search fields round 10), 14 (the sheet).
- **Spacing:** a 4 px scale (4, 8, 12, 16, 24, 32).
- **Type:** the platform default UI font (or the family selected in Appearance) at 13 px body, 12 px secondary, 15 px headings and 24 px page titles in its display cut. Labels are sentence case. [ADR-0029](adr/0029-private-fonts-and-nerd-symbols.md) adds independent app/terminal family selection, searchable installed/bundled/imported families, private TTF/OTF/TTC import by path or drag and drop, and live samples. JetBrains Mono is the terminal default, Fira Mono is an alternate, and both include curated OFL Nerd symbols without OS installation. Import/status messages stay in the font card.
- **Density:** a 32 px control height, a 40 px search field at the top of a screen, and 64 px host cards in grid view (36 px rows in list view).
- **Elevation:** cards and rows are a step off the sheet with a faint edge (`Ui::card_surface`), and fields have no outline at rest. Floating surfaces (`Ui::raised_surface`) have a wide soft shadow, a tight one under it and a hairline.
- **Scroll bars** sit in the page's margin (`SCROLL_GUTTER`), so content keeps one right edge scrolling or not, and show only while the pointer is over the list or the list is moving.
- **Icons:** our own 16/20 px line icon set, drawn as vector paths (1.5 px stroke, rounded caps).
- **Mark** (`annulus_ui::mark`, our own design, drawn as vectors): a dark rounded tile with a thin outline, holding a ring of segments open at the bottom, three chevrons on it pointing in (at the top and a little above either side), a letter A without its bar whose legs run out through the opening, and a four-pointed spark under its peak. It brings its own tile, so it keeps its own colors in every theme and color scheme (`tokens::MARK`): tile #0D0D0F, ring and outline #34363A, letter #DCDBDC, and a teal #64E7D5 for the chevrons and the spark, also used as the dark theme’s accent. The letter and the teal stand at 7:1 or better on the tile (`the_letter_and_the_light_stand_out_from_the_tile`). One set of shapes draws it in the title bar, on the lock screen and in the film, and as the app icon and the README's logo (`cargo xtask icon`; on macOS on Apple's icon grid). At 16 and 20 px the letter and the chevrons carry it, and the ring fades to a faint arc.
- **Motion** (`tokens::motion`, `annulus_ui::motion`, [ADR-0021](adr/0021-quiet-look-and-choreographed-motion.md)): every change animates, quickly. The pointer is answered in 70 ms and let go in 220 ms; 120 ms for small fades and focus; 220 ms for menus, page and tab changes; 320 ms for large surfaces; rows of a list filling in start 28 ms apart (at most ten steps). What arrives moves on an emphasized curve (quick to start, a long soft landing); what leaves accelerates away in 90 ms (200 ms for a side panel) and takes no input meanwhile, while what replaced it already works. Things that pop in overshoot by at most 5 %. Nothing big just appears: a card on a list page (a new key, a snippet, a rule, an install) unfolds into the page over 320 ms with the list gliding down out of its way, and folds away in 200 ms with its secrets already wiped; a new row rises in with the accent on it for a moment, a row that went fades where it was while the others close up, and an empty state fades in; dialogs grow over a fading scrim and leave the same way; wizard steps come in from the side they lie on; another group or other tags fade the old hosts out before the new ones fill in. `nothing_big_appears_at_once` measures this from the pixels of every scene. Tabs and scrolling follow a critically damped spring that settles in about 0.2 s; selections and cards moving to new places follow a soft one that settles in about 0.4 s, passing their target by about 1 %. A change of mind mid-way continues from where the value is, never jumping. Layers move, scale and fade what they hold together with its click targets; a surface whose shapes overlap fades as one group. Terminal text is never animated. Frames are asked for only while something moves: a scene fails if anything still asks one two seconds after the last input. Settings → Appearance → Reduce motion puts every change at its end at once.
- **Default terminal theme:** "Annulus Dark" / "Annulus Light" (our own palettes). Community schemes ship with their licenses recorded.
- **Color schemes** ([ADR-0028](adr/0028-theme-browser.md)): Annulus’s own default, all 498 tinted-theming schemes and six built-in palettes. Appearance → Color scheme opens a modal with search, All/Dark/Light filters, family headings and a scrolling family index, a Recent group, a result count and a sample server list and a real terminal preview with the highlighted theme’s ANSI palette. Moving the pointer onto a row starts its preview on the next frame, arrows move the highlight and scroll it into view, and colors cross-fade. Click a row, press Enter or choose Apply theme to save; Escape, Close or the backdrop cancel and restore focus. Background controls are inert. Previewing and filtering never save preferences. The initial filter follows the Appearance mode; applying a scheme aligns that mode to its variant. The explicit Theme mode tabs still drop a scheme of the other kind (ADR-0026). Terminal → Colors remains a separate searchable menu with swatches and its own terminal preview. Community and built-in palettes use the same WCAG AA checks as Annulus’s default. Reduce motion skips transitions.

## Accessibility

- Everything is reachable by keyboard, Tab order follows the visual order, and focus is always visible.
- A focus ring is drawn whole. It reaches `FOCUS_REACH` (5 px) past its widget, so a scrolled list clips with `Ui::push_list_clip`, which leaves that room on the sides and at whichever end the list is scrolled to. A list that runs edge to edge in its panel draws the ring inside the row instead. Every scene fails on a settled frame in which a clip cuts off the ring of a widget in full view.
- Every widget exposes a role, a name and its state through AccessKit (UIA, NSAccessibility, AT-SPI). The terminal exposes its text as a document.
- Text contrast meets WCAG AA in both themes. Nothing is conveyed by color alone: status dots also have a tooltip and an accessible name.
- The UI scale follows the OS setting and can be changed from 80% to 200% in Settings.
- The release gate is a manual pass with NVDA, VoiceOver and Orca.

//! Every keyboard shortcut (UI-SPEC §7) in one table: the app checks for each one through it,
//! and Settings → Shortcuts lists it, so what the list says is what the keys do. Each one has
//! keys for a PC's keyboard and for a Mac's ([`Keyboard`]), and this computer's are the ones
//! that count. On a PC, Ctrl with a letter belongs to the shell, so app chords add Shift; on a
//! Mac, Cmd is free, so app chords hold Cmd.
//!
//! Settings can give a shortcut other keys (F-SET-004). The table stays constant; the keys
//! chosen sit beside it for the UI thread, read through [`Shortcut::current`].

use std::cell::RefCell;

use annulus_config::settings::KeyBinding;
use annulus_ui::kit::{Key, Modifiers, Ui};

/// The keys chosen in Settings as they were last taken, and what they mean.
#[derive(Default)]
struct Chosen {
    bindings: Vec<KeyBinding>,
    chords: Vec<(&'static str, (Key, Modifiers))>,
}

thread_local! {
    static CHOSEN: RefCell<Chosen> = RefCell::new(Chosen::default());
}

/// Takes the keys chosen in Settings: each shortcut named answers to its new keys from now on,
/// and no longer to its own. A name that is no shortcut here, or keys that do not read, are
/// left out. Cheap when nothing changed, so the app calls it every frame.
pub fn choose(bindings: &[KeyBinding]) {
    CHOSEN.with(|chosen| {
        let mut chosen = chosen.borrow_mut();
        if chosen.bindings == bindings {
            return;
        }
        chosen.bindings = bindings.to_vec();
        chosen.chords = bindings
            .iter()
            .filter_map(|binding| {
                let shortcut = ALL
                    .iter()
                    .find(|shortcut| shortcut.own().is_some() && shortcut.id == binding.action)?;
                Some((shortcut.id, parse_chord(&binding.keys)?))
            })
            .collect();
    });
}

/// The keyboard a shortcut's keys are for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keyboard {
    /// Windows and Linux: Ctrl, Alt, Shift and the Windows key.
    Pc,
    /// macOS: Cmd, Ctrl, Option and Shift.
    Mac,
}

impl Keyboard {
    /// This computer's.
    pub const HOST: Keyboard = if cfg!(target_os = "macos") {
        Keyboard::Mac
    } else {
        Keyboard::Pc
    };

    /// Both, for the tests that hold every keyboard's table to the same rules.
    #[cfg(test)]
    const BOTH: [Keyboard; 2] = [Keyboard::Pc, Keyboard::Mac];

    /// The keys held, as this keyboard names them: "Ctrl+Alt+Shift" on a PC, "Cmd+Option" on
    /// a Mac (in the order the keyboard's own menus write them).
    fn held(self, modifiers: Modifiers) -> Vec<&'static str> {
        let names = match self {
            Keyboard::Pc => [
                (modifiers.ctrl, "Ctrl"),
                (modifiers.alt, "Alt"),
                (modifiers.shift, "Shift"),
                (modifiers.logo, "Win"),
            ],
            Keyboard::Mac => [
                (modifiers.logo, "Cmd"),
                (modifiers.ctrl, "Ctrl"),
                (modifiers.alt, "Option"),
                (modifiers.shift, "Shift"),
            ],
        };
        names
            .into_iter()
            .filter_map(|(held, name)| held.then_some(name))
            .collect()
    }

    /// `key` with `modifiers` written out: "Ctrl+Alt+Shift+B" on a PC, "Cmd+Option+Shift+B"
    /// on a Mac.
    #[must_use]
    pub fn write(self, key: Key, modifiers: Modifiers) -> String {
        let mut parts: Vec<String> = self
            .held(modifiers)
            .into_iter()
            .map(str::to_owned)
            .collect();
        parts.push(match key {
            Key::Char(c) => c.to_uppercase().to_string(),
            Key::F(n) => format!("F{n}"),
            Key::PageUp => "PageUp".into(),
            Key::PageDown => "PageDown".into(),
            other => format!("{other:?}"),
        });
        parts.join("+")
    }

    /// `keys` written out: a chord as [`Keyboard::write`] has it, "Alt+1…9" for digits held
    /// with Alt, "Ctrl+click" for a click held with Ctrl.
    fn describe(self, keys: Keys) -> String {
        let with = |held: Modifiers, rest: &str| {
            let mut parts = self.held(held);
            parts.push(rest);
            parts.join("+")
        };
        match keys {
            Keys::Chord(key, modifiers) => self.write(key, modifiers),
            Keys::Digits(held) => with(held, "1\u{2026}9"),
            Keys::Arrows(held) => with(held, "Arrows"),
            Keys::Click(held) => with(held, "click"),
            Keys::Shown(text) => text.to_owned(),
        }
    }

    /// Whether shortcut `id` may answer to `key` with `modifiers` on this keyboard; if not,
    /// why, in a few words.
    ///
    /// # Errors
    /// The keys would take typing from the terminal, or another shortcut has them.
    pub fn check(self, id: &str, key: Key, modifiers: Modifiers) -> Result<(), String> {
        let own = ALL
            .iter()
            .find(|shortcut| shortcut.id == id)
            .and_then(|shortcut| shortcut.own_on(self));
        // On a Mac, Option with a key types a character (é, ß, …), so it is no app key alone.
        let (held, needed) = match self {
            Keyboard::Pc => (
                modifiers.ctrl || modifiers.alt || modifiers.logo,
                "Ctrl or Alt",
            ),
            Keyboard::Mac => (modifiers.ctrl || modifiers.logo, "Cmd"),
        };
        if !held && !matches!(key, Key::F(_)) {
            return Err(format!(
                "Hold {needed} too, so typing still reaches the terminal."
            ));
        }
        let shell =
            modifiers == Modifiers::CTRL && matches!(key, Key::Char(c) if c.is_alphabetic());
        if shell && own != Some((key, modifiers)) {
            return Err(match self {
                Keyboard::Pc => "Ctrl with a letter belongs to the shell; add Shift.".into(),
                Keyboard::Mac => "Ctrl with a letter belongs to the shell; use Cmd.".into(),
            });
        }
        if let Some(user) = ALL
            .iter()
            .find(|shortcut| shortcut.id != id && shortcut.takes(self, key, modifiers))
        {
            return Err(format!("Already used by \u{201c}{}\u{201d}.", user.action));
        }
        Ok(())
    }
}

/// A shortcut's keys on one keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keys {
    /// One chord, which Settings can change.
    Chord(Key, Modifiers),
    /// These held with a digit from 1 to 9.
    Digits(Modifiers),
    /// These held with an arrow.
    Arrows(Modifiers),
    /// These held with a click.
    Click(Modifiers),
    /// Keys a widget or the window handles, as the list writes them.
    Shown(&'static str),
}

/// One shortcut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shortcut {
    /// The group it is listed under.
    pub group: &'static str,
    /// What it does.
    pub action: &'static str,
    /// Its keys on a PC's keyboard.
    pc: Keys,
    /// Its keys on a Mac's.
    mac: Keys,
    /// Its name here, so a test can tell the app checks for it; Settings stores a chord
    /// chosen for it under this name.
    pub id: &'static str,
}

const CTRL: Modifiers = Modifiers::CTRL;
const CTRL_SHIFT: Modifiers = Modifiers::CTRL_SHIFT;
const ALT: Modifiers = Modifiers {
    alt: true,
    ..Modifiers::NONE
};
const CTRL_ALT: Modifiers = Modifiers {
    ctrl: true,
    alt: true,
    ..Modifiers::NONE
};
const CTRL_ALT_SHIFT: Modifiers = Modifiers {
    ctrl: true,
    alt: true,
    shift: true,
    ..Modifiers::NONE
};
const CMD: Modifiers = Modifiers {
    logo: true,
    ..Modifiers::NONE
};
const CMD_SHIFT: Modifiers = Modifiers {
    logo: true,
    shift: true,
    ..Modifiers::NONE
};
const CMD_CTRL: Modifiers = Modifiers {
    logo: true,
    ctrl: true,
    ..Modifiers::NONE
};
const CMD_OPTION: Modifiers = Modifiers {
    logo: true,
    alt: true,
    ..Modifiers::NONE
};
const CMD_OPTION_SHIFT: Modifiers = Modifiers {
    logo: true,
    alt: true,
    shift: true,
    ..Modifiers::NONE
};

impl Shortcut {
    /// One with a chord on each keyboard: `pc`, then `mac`.
    const fn chord(
        group: &'static str,
        action: &'static str,
        id: &'static str,
        pc: (Key, Modifiers),
        mac: (Key, Modifiers),
    ) -> Self {
        Self {
            group,
            action,
            pc: Keys::Chord(pc.0, pc.1),
            mac: Keys::Chord(mac.0, mac.1),
            id,
        }
    }

    /// One listed but never changed: `pc`, then `mac`.
    const fn described(
        group: &'static str,
        action: &'static str,
        id: &'static str,
        pc: Keys,
        mac: Keys,
    ) -> Self {
        Self {
            group,
            action,
            pc,
            mac,
            id,
        }
    }

    /// Its keys on `keyboard`.
    #[must_use]
    pub const fn keys_on(&self, keyboard: Keyboard) -> Keys {
        match keyboard {
            Keyboard::Pc => self.pc,
            Keyboard::Mac => self.mac,
        }
    }

    /// Its own chord on `keyboard`; `None` for one only described there.
    #[must_use]
    pub const fn own_on(&self, keyboard: Keyboard) -> Option<(Key, Modifiers)> {
        match self.keys_on(keyboard) {
            Keys::Chord(key, modifiers) => Some((key, modifiers)),
            _ => None,
        }
    }

    /// Its own chord on this computer's keyboard.
    #[must_use]
    pub const fn own(&self) -> Option<(Key, Modifiers)> {
        self.own_on(Keyboard::HOST)
    }

    /// Its chord on `keyboard` now: the keys chosen in Settings, or its own.
    fn current_on(&self, keyboard: Keyboard) -> Option<(Key, Modifiers)> {
        let own = self.own_on(keyboard)?;
        let chosen = CHOSEN.with(|chosen| {
            chosen
                .borrow()
                .chords
                .iter()
                .find(|(id, _)| *id == self.id)
                .map(|(_, chord)| *chord)
        });
        Some(chosen.unwrap_or(own))
    }

    /// Its chord now: the keys chosen in Settings, or its own.
    #[must_use]
    pub fn current(&self) -> Option<(Key, Modifiers)> {
        self.current_on(Keyboard::HOST)
    }

    /// Whether Settings gave it other keys than its own.
    #[must_use]
    pub fn changed(&self) -> bool {
        self.current() != self.own()
    }

    /// Whether its chord was pressed this frame (and takes it).
    pub fn pressed(&self, ui: &mut Ui<'_>) -> bool {
        self.current()
            .is_some_and(|(key, modifiers)| ui.key(key, modifiers))
    }

    /// For one held with a digit or an arrow: whether it was pressed with `key` this frame
    /// (and takes it).
    pub fn pressed_with(&self, ui: &mut Ui<'_>, key: Key) -> bool {
        match self.keys_on(Keyboard::HOST) {
            Keys::Digits(held) if matches!(key, Key::Char('1'..='9')) => ui.key(key, held),
            Keys::Arrows(held) if is_arrow(key) => ui.key(key, held),
            _ => false,
        }
    }

    /// For one held with a click: whether `modifiers` hold its keys.
    #[must_use]
    pub fn held_by(&self, modifiers: Modifiers) -> bool {
        let Keys::Click(held) = self.keys_on(Keyboard::HOST) else {
            return false;
        };
        (!held.ctrl || modifiers.ctrl)
            && (!held.alt || modifiers.alt)
            && (!held.shift || modifiers.shift)
            && (!held.logo || modifiers.logo)
    }

    /// Whether `key` with `modifiers` is its chord.
    #[must_use]
    pub fn is(&self, key: Key, modifiers: Modifiers) -> bool {
        self.current() == Some((key, modifiers))
    }

    /// The keys as the list shows them: "Ctrl+Shift+T" on a PC, "Cmd+T" on a Mac.
    #[must_use]
    pub fn keys(&self) -> String {
        match self.current() {
            Some((key, modifiers)) => chord_text(key, modifiers),
            None => Keyboard::HOST.describe(self.keys_on(Keyboard::HOST)),
        }
    }

    /// Whether it answers to `key` with `modifiers` on `keyboard` now, the described ones
    /// included.
    fn takes(&self, keyboard: Keyboard, key: Key, modifiers: Modifiers) -> bool {
        match self.keys_on(keyboard) {
            Keys::Chord(..) => self.current_on(keyboard) == Some((key, modifiers)),
            Keys::Digits(held) => modifiers == held && matches!(key, Key::Char('1'..='9')),
            Keys::Arrows(held) => modifiers == held && is_arrow(key),
            Keys::Click(_) => false,
            Keys::Shown(text) => parse_chord(text) == Some((key, modifiers)),
        }
    }
}

fn is_arrow(key: Key) -> bool {
    matches!(key, Key::Left | Key::Right | Key::Up | Key::Down)
}

/// Whether shortcut `id` may answer to `key` with `modifiers` on this computer's keyboard; if
/// not, why, in a few words.
///
/// # Errors
/// The keys would take typing from the terminal, or another shortcut has them.
pub fn check(id: &str, key: Key, modifiers: Modifiers) -> Result<(), String> {
    Keyboard::HOST.check(id, key, modifiers)
}

/// The keys [`chord_text`] wrote, read back; `None` for text that is no chord. Either
/// keyboard's names read: "Win" and "Cmd" are the same key, and so are "Alt" and "Option".
#[must_use]
pub fn parse_chord(text: &str) -> Option<(Key, Modifiers)> {
    // The key itself can be "+", which leaves "++" at the end.
    let (held, key) = match text.strip_suffix("++") {
        Some(held) => (held, "+"),
        None => text.rsplit_once('+').unwrap_or(("", text)),
    };
    let mut modifiers = Modifiers::NONE;
    for name in held.split('+').filter(|name| !name.is_empty()) {
        match name {
            "Ctrl" => modifiers.ctrl = true,
            "Alt" | "Option" => modifiers.alt = true,
            "Shift" => modifiers.shift = true,
            "Win" | "Cmd" => modifiers.logo = true,
            _ => return None,
        }
    }
    let named = [
        Key::Tab,
        Key::Enter,
        Key::Escape,
        Key::Space,
        Key::Backspace,
        Key::Delete,
        Key::Left,
        Key::Right,
        Key::Up,
        Key::Down,
        Key::Home,
        Key::End,
        Key::PageUp,
        Key::PageDown,
    ];
    let key = if let Some(found) = named.into_iter().find(|named| format!("{named:?}") == key) {
        found
    } else if let Some(n) = key.strip_prefix('F').and_then(|n| n.parse::<u8>().ok()) {
        Key::F(n)
    } else {
        let mut chars = key.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) => Key::Char(c.to_lowercase().next().unwrap_or(c)),
            _ => return None,
        }
    };
    Some((key, modifiers))
}

/// `key` with `modifiers` written out as this computer's keyboard names them:
/// "Ctrl+Alt+Shift+B" on a PC, "Cmd+Option+Shift+B" on a Mac.
#[must_use]
pub fn chord_text(key: Key, modifiers: Modifiers) -> String {
    Keyboard::HOST.write(key, modifiers)
}

// Tabs.
/// A New Tab page.
pub const NEW_TAB: Shortcut = Shortcut::chord(
    "Tabs",
    "New tab",
    "NEW_TAB",
    (Key::Char('t'), CTRL_SHIFT),
    (Key::Char('t'), CMD),
);
/// A local terminal with the default shell.
pub const LOCAL: Shortcut = Shortcut::chord(
    "Tabs",
    "Local terminal",
    "LOCAL",
    (Key::Char('l'), CTRL_SHIFT),
    (Key::Char('l'), CMD),
);
/// Close the pane in front, or its tab.
pub const CLOSE: Shortcut = Shortcut::chord(
    "Tabs",
    "Close the pane or tab",
    "CLOSE",
    (Key::Char('w'), CTRL_SHIFT),
    (Key::Char('w'), CMD),
);
/// The next tab.
pub const NEXT_TAB: Shortcut = Shortcut::chord(
    "Tabs",
    "Next tab",
    "NEXT_TAB",
    (Key::Tab, CTRL),
    (Key::Char(']'), CMD_SHIFT),
);
/// The tab before.
pub const PREVIOUS_TAB: Shortcut = Shortcut::chord(
    "Tabs",
    "Previous tab",
    "PREVIOUS_TAB",
    (Key::Tab, CTRL_SHIFT),
    (Key::Char('['), CMD_SHIFT),
);
/// Go to a tab by its place.
pub const GO_TO_TAB: Shortcut = Shortcut::described(
    "Tabs",
    "Go to tab 1 to 9",
    "GO_TO_TAB",
    Keys::Digits(ALT),
    Keys::Digits(CMD),
);
/// Move the tab in front one place left.
pub const MOVE_LEFT: Shortcut = Shortcut::chord(
    "Tabs",
    "Move the tab left",
    "MOVE_LEFT",
    (Key::PageUp, CTRL_SHIFT),
    (Key::Char('['), CMD_OPTION_SHIFT),
);
/// Move the tab in front one place right.
pub const MOVE_RIGHT: Shortcut = Shortcut::chord(
    "Tabs",
    "Move the tab right",
    "MOVE_RIGHT",
    (Key::PageDown, CTRL_SHIFT),
    (Key::Char(']'), CMD_OPTION_SHIFT),
);

// Panes.
/// Split side by side.
pub const SPLIT_RIGHT: Shortcut = Shortcut::chord(
    "Panes",
    "Split to the right",
    "SPLIT_RIGHT",
    (Key::Char('d'), CTRL_SHIFT),
    (Key::Char('d'), CMD),
);
/// Split one above the other.
pub const SPLIT_DOWN: Shortcut = Shortcut::chord(
    "Panes",
    "Split downwards",
    "SPLIT_DOWN",
    (Key::Char('e'), CTRL_SHIFT),
    (Key::Char('d'), CMD_SHIFT),
);
/// Move between panes.
pub const PANE_FOCUS: Shortcut = Shortcut::described(
    "Panes",
    "Move between panes",
    "PANE_FOCUS",
    Keys::Arrows(ALT),
    Keys::Arrows(CMD_OPTION),
);
/// One large pane and the others listed beside it.
pub const FOCUS_MODE: Shortcut = Shortcut::chord(
    "Panes",
    "Focus mode",
    "FOCUS_MODE",
    (Key::Char('m'), CTRL_SHIFT),
    (Key::Char('m'), CMD_SHIFT),
);
/// Type into every pane at once.
pub const BROADCAST: Shortcut = Shortcut::chord(
    "Panes",
    "Type into every pane",
    "BROADCAST",
    (Key::Char('b'), CTRL_SHIFT),
    (Key::Char('b'), CMD_SHIFT),
);
/// Leave the pane in front out of broadcast, or back in.
pub const LEAVE_OUT: Shortcut = Shortcut::chord(
    "Panes",
    "Leave this pane out of typing into every pane",
    "LEAVE_OUT",
    (Key::Char('b'), CTRL_ALT_SHIFT),
    (Key::Char('b'), CMD_OPTION_SHIFT),
);
/// The snippets beside the terminal.
pub const SNIPPETS: Shortcut = Shortcut::chord(
    "Panes",
    "Snippets beside the terminal",
    "SNIPPETS",
    (Key::Char('.'), CTRL_SHIFT),
    (Key::Char('.'), CMD),
);

// Terminal.
/// Copy the selection.
pub const COPY: Shortcut = Shortcut::chord(
    "Terminal",
    "Copy",
    "COPY",
    (Key::Char('c'), CTRL_SHIFT),
    (Key::Char('c'), CMD),
);
/// Paste (the window handles it, and Shift+Insert too).
pub const PASTE: Shortcut = Shortcut::described(
    "Terminal",
    "Paste",
    "",
    Keys::Shown("Ctrl+Shift+V"),
    Keys::Shown("Cmd+V"),
);
/// Find in the output.
pub const FIND: Shortcut = Shortcut::chord(
    "Terminal",
    "Find",
    "FIND",
    (Key::Char('f'), CTRL_SHIFT),
    (Key::Char('f'), CMD),
);
/// Clear what scrolled away.
pub const CLEAR: Shortcut = Shortcut::chord(
    "Terminal",
    "Clear scrollback",
    "CLEAR",
    (Key::Backspace, CTRL_SHIFT),
    (Key::Char('k'), CMD_SHIFT),
);
/// Open the link under the pointer.
pub const OPEN_LINK: Shortcut = Shortcut::described(
    "Terminal",
    "Open a link",
    "OPEN_LINK",
    Keys::Click(CTRL),
    Keys::Click(CMD),
);
/// Larger text.
pub const ZOOM_IN: Shortcut = Shortcut::chord(
    "Terminal",
    "Larger text",
    "ZOOM_IN",
    (Key::Char('='), CTRL),
    (Key::Char('='), CMD),
);
/// Smaller text.
pub const ZOOM_OUT: Shortcut = Shortcut::chord(
    "Terminal",
    "Smaller text",
    "ZOOM_OUT",
    (Key::Char('-'), CTRL),
    (Key::Char('-'), CMD),
);
/// The text size from Settings.
pub const ZOOM_RESET: Shortcut = Shortcut::chord(
    "Terminal",
    "Text back to 14 pt",
    "ZOOM_RESET",
    (Key::Char('0'), CTRL),
    (Key::Char('0'), CMD),
);

// Hosts.
/// A new host in the vault.
pub const NEW_HOST: Shortcut = Shortcut::chord(
    "Hosts",
    "New host",
    "NEW_HOST",
    (Key::Char('n'), CTRL_SHIFT),
    (Key::Char('n'), CMD),
);
/// Save the host being edited.
pub const SAVE: Shortcut = Shortcut::chord(
    "Hosts",
    "Save the host",
    "SAVE",
    (Key::Char('s'), CTRL),
    (Key::Char('s'), CMD),
);
/// A host's menu (the toolkit opens context menus).
pub const MENU: Shortcut = Shortcut::described(
    "Hosts",
    "A host's menu",
    "",
    Keys::Shown("Shift+F10"),
    Keys::Shown("Shift+F10"),
);
/// Connect to the host chosen.
pub const CONNECT: Shortcut = Shortcut::described(
    "Hosts",
    "Connect to the host chosen",
    "",
    Keys::Shown("Enter"),
    Keys::Shown("Enter"),
);

// App.
/// The command palette.
pub const PALETTE: Shortcut = Shortcut::chord(
    "App",
    "Command palette",
    "PALETTE",
    (Key::Char('p'), CTRL_SHIFT),
    (Key::Char('p'), CMD_SHIFT),
);
/// Settings.
pub const SETTINGS: Shortcut = Shortcut::chord(
    "App",
    "Settings",
    "SETTINGS",
    (Key::Char(','), CTRL),
    (Key::Char(','), CMD),
);
/// Lock the vault.
pub const LOCK: Shortcut = Shortcut::chord(
    "App",
    "Lock the vault",
    "LOCK",
    (Key::Char('l'), CTRL_ALT),
    (Key::Char('l'), CMD_CTRL),
);

/// Every shortcut, in the order Settings lists them.
pub const ALL: [Shortcut; 30] = [
    NEW_TAB,
    LOCAL,
    CLOSE,
    NEXT_TAB,
    PREVIOUS_TAB,
    GO_TO_TAB,
    MOVE_LEFT,
    MOVE_RIGHT,
    SPLIT_RIGHT,
    SPLIT_DOWN,
    PANE_FOCUS,
    FOCUS_MODE,
    BROADCAST,
    LEAVE_OUT,
    SNIPPETS,
    COPY,
    PASTE,
    FIND,
    CLEAR,
    OPEN_LINK,
    ZOOM_IN,
    ZOOM_OUT,
    ZOOM_RESET,
    NEW_HOST,
    SAVE,
    MENU,
    CONNECT,
    PALETTE,
    SETTINGS,
    LOCK,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_chord_is_one_of_a_kind_and_the_app_checks_for_it() {
        let sources = [
            include_str!("mod.rs"),
            include_str!("live.rs"),
            include_str!("hosts.rs"),
        ]
        .concat();
        for keyboard in Keyboard::BOTH {
            for shortcut in ALL {
                let keys = shortcut.keys_on(keyboard);
                assert_eq!(
                    std::mem::discriminant(&keys),
                    std::mem::discriminant(&shortcut.keys_on(Keyboard::Pc)),
                    "{} is listed alike on both keyboards",
                    shortcut.action
                );
                if let Keys::Shown(text) = keys {
                    assert!(!text.is_empty(), "{}", shortcut.action);
                    continue;
                }
                if let Some((key, modifiers)) = shortcut.own_on(keyboard) {
                    let other = ALL.iter().find(|other| {
                        other.id != shortcut.id && other.takes(keyboard, key, modifiers)
                    });
                    assert_eq!(
                        other.map(|other| other.action),
                        None,
                        "{} shares its keys on a {keyboard:?}",
                        shortcut.action
                    );
                }
                assert!(
                    sources.contains(&format!("shortcuts::{}", shortcut.id)),
                    "nothing checks for {} ({})",
                    shortcut.action,
                    keyboard.describe(keys)
                );
            }
        }
    }

    #[test]
    fn no_screen_writes_out_the_keys_of_a_shortcut_that_can_change() {
        // Words naming such keys ask the table, or they go stale once Settings changes them.
        let sources = [
            ("account.rs", include_str!("account.rs")),
            ("ask.rs", include_str!("ask.rs")),
            ("hosts.rs", include_str!("hosts.rs")),
            ("live.rs", include_str!("live.rs")),
            ("lock.rs", include_str!("lock.rs")),
            ("mod.rs", include_str!("mod.rs")),
            ("pages.rs", include_str!("pages.rs")),
            ("palette.rs", include_str!("palette.rs")),
            ("panes.rs", include_str!("panes.rs")),
            ("screens.rs", include_str!("screens.rs")),
            ("settings.rs", include_str!("settings.rs")),
            ("sftp.rs", include_str!("sftp.rs")),
            ("start.rs", include_str!("start.rs")),
            ("terminal.rs", include_str!("terminal.rs")),
            ("titlebar.rs", include_str!("titlebar.rs")),
            ("command_palette.rs", include_str!("../command_palette.rs")),
            ("sync_screen.rs", include_str!("../sync_screen.rs")),
            ("terminal_view.rs", include_str!("../terminal_view.rs")),
        ];
        let written: Vec<(String, &str)> = Keyboard::BOTH
            .into_iter()
            .flat_map(|keyboard| {
                ALL.iter().filter_map(move |shortcut| {
                    let (key, modifiers) = shortcut.own_on(keyboard)?;
                    Some((keyboard.write(key, modifiers), shortcut.id))
                })
            })
            .collect();
        for (file, source) in sources {
            let code = source.split("#[cfg(test)]").next().unwrap_or(source);
            let lines = code.lines().enumerate();
            for (number, line) in lines.filter(|(_, line)| !line.trim_start().starts_with("//")) {
                for (keys, id) in &written {
                    let mut rest = line;
                    while let Some(at) = rest.find(keys.as_str()) {
                        let before = rest[..at].chars().next_back();
                        let after = rest[at + keys.len()..].chars().next();
                        let joined = before.is_some_and(|c| c.is_alphanumeric() || c == '+')
                            || after.is_some_and(char::is_alphanumeric);
                        assert!(
                            joined,
                            "{file}:{}: {keys} written out; ask shortcuts::{id}.keys()",
                            number + 1,
                        );
                        rest = &rest[at + keys.len()..];
                    }
                }
            }
        }
    }

    #[test]
    fn keys_chosen_in_settings_replace_a_shortcuts_own_until_they_are_dropped() {
        let binding = |action: &str, keys: &str| KeyBinding {
            action: action.into(),
            keys: keys.into(),
        };
        let own = NEW_TAB.own().unwrap();
        choose(&[
            binding("NEW_TAB", "Ctrl+Alt+N"),
            binding("NO_SUCH_SHORTCUT", "Ctrl+Alt+X"),
            binding("LOCAL", "not keys"),
            binding("GO_TO_TAB", "Ctrl+Alt+G"),
        ]);
        assert!(NEW_TAB.is(Key::Char('n'), CTRL_ALT));
        assert!(!NEW_TAB.is(own.0, own.1), "its own keys no longer");
        assert_eq!(NEW_TAB.keys(), chord_text(Key::Char('n'), CTRL_ALT));
        assert!(NEW_TAB.changed());
        assert!(!LOCAL.changed(), "keys that do not read leave it alone");
        assert_eq!(
            GO_TO_TAB.current(),
            None,
            "one only described takes no keys"
        );
        // The keys it gave up are free; the ones it took are not.
        assert_eq!(check("LOCAL", own.0, own.1), Ok(()));
        assert!(
            check("LOCAL", Key::Char('n'), CTRL_ALT)
                .unwrap_err()
                .contains("New tab")
        );
        choose(&[]);
        assert!(NEW_TAB.is(own.0, own.1));
        assert!(!NEW_TAB.changed());
    }

    #[test]
    fn every_chord_reads_back_from_how_it_is_written() {
        for keyboard in Keyboard::BOTH {
            for shortcut in ALL {
                if let Some((key, modifiers)) = shortcut.own_on(keyboard) {
                    assert_eq!(
                        parse_chord(&keyboard.write(key, modifiers)),
                        Some((key, modifiers)),
                        "{} on a {keyboard:?}",
                        shortcut.action
                    );
                }
            }
        }
        assert_eq!(parse_chord("Ctrl++"), Some((Key::Char('+'), CTRL)));
        assert_eq!(parse_chord("F5"), Some((Key::F(5), Modifiers::NONE)));
        // A Mac's names and a PC's name the same keys.
        assert_eq!(parse_chord("Cmd+Option+K"), parse_chord("Win+Alt+K"));
        assert_eq!(parse_chord("Hyper+K"), None);
        assert_eq!(parse_chord("Ctrl+Shift+Banana"), None);
    }

    #[test]
    fn keys_that_would_take_typing_or_belong_to_another_are_refused() {
        let refused = |key, modifiers| Keyboard::Pc.check("NEW_TAB", key, modifiers).unwrap_err();
        assert!(refused(Key::Char('p'), CTRL_SHIFT).contains("Command palette"));
        assert!(refused(Key::Char('n'), Modifiers::SHIFT).contains("typing"));
        assert!(refused(Key::Char('r'), CTRL).contains("add Shift"));
        assert!(refused(Key::Char('3'), ALT).contains("Go to tab"));
        assert!(refused(Key::Left, ALT).contains("between panes"));
        assert!(refused(Key::F(10), Modifiers::SHIFT).contains("menu"));
        assert!(refused(Key::Char('v'), CTRL_SHIFT).contains("Paste"));
        let pc = |id, key, modifiers| Keyboard::Pc.check(id, key, modifiers);
        assert_eq!(pc("NEW_TAB", Key::F(6), Modifiers::NONE), Ok(()));
        assert_eq!(pc("NEW_TAB", Key::Char('n'), CTRL_ALT), Ok(()));
        // A shortcut's own keys clash with nothing, Ctrl+S for saving included.
        assert_eq!(pc("NEW_TAB", Key::Char('t'), CTRL_SHIFT), Ok(()));
        assert_eq!(pc("SAVE", Key::Char('s'), CTRL), Ok(()));
    }

    #[test]
    fn on_a_mac_keys_must_hold_cmd_and_leave_ctrl_letters_to_the_shell() {
        let refused = |key, modifiers| Keyboard::Mac.check("NEW_TAB", key, modifiers).unwrap_err();
        assert!(refused(Key::Char('p'), CMD_SHIFT).contains("Command palette"));
        // Option with a key types a character on a Mac.
        assert!(refused(Key::Char('n'), ALT).contains("Hold Cmd"));
        assert!(refused(Key::Char('r'), CTRL).contains("use Cmd"));
        assert!(refused(Key::Char('3'), CMD).contains("Go to tab"));
        assert!(refused(Key::Left, CMD_OPTION).contains("between panes"));
        assert!(refused(Key::Char('v'), CMD).contains("Paste"));
        let mac = |key, modifiers| Keyboard::Mac.check("NEW_TAB", key, modifiers);
        assert_eq!(mac(Key::Char('n'), CMD_OPTION), Ok(()));
        assert_eq!(mac(Key::Char('t'), CMD), Ok(()), "its own");
        // A PC's chords are free there, and the other way round.
        assert_eq!(mac(Key::Char('p'), CTRL_SHIFT), Ok(()));
        assert_eq!(
            Keyboard::Pc.check("NEW_TAB", Key::Char('p'), CMD_SHIFT),
            Ok(())
        );
    }

    #[test]
    fn chords_are_written_the_way_the_keyboard_says_them() {
        let written =
            |keyboard: Keyboard, shortcut: Shortcut| keyboard.describe(shortcut.keys_on(keyboard));
        for (shortcut, pc, mac) in [
            (NEW_TAB, "Ctrl+Shift+T", "Cmd+T"),
            (LEAVE_OUT, "Ctrl+Alt+Shift+B", "Cmd+Option+Shift+B"),
            (MOVE_LEFT, "Ctrl+Shift+PageUp", "Cmd+Option+Shift+["),
            (NEXT_TAB, "Ctrl+Tab", "Cmd+Shift+]"),
            (ZOOM_IN, "Ctrl+=", "Cmd+="),
            (CLEAR, "Ctrl+Shift+Backspace", "Cmd+Shift+K"),
            (LOCK, "Ctrl+Alt+L", "Cmd+Ctrl+L"),
            (GO_TO_TAB, "Alt+1\u{2026}9", "Cmd+1\u{2026}9"),
            (PANE_FOCUS, "Alt+Arrows", "Cmd+Option+Arrows"),
            (OPEN_LINK, "Ctrl+click", "Cmd+click"),
            (PASTE, "Ctrl+Shift+V", "Cmd+V"),
            (PALETTE, "Ctrl+Shift+P", "Cmd+Shift+P"),
        ] {
            assert_eq!(written(Keyboard::Pc, shortcut), pc);
            assert_eq!(written(Keyboard::Mac, shortcut), mac);
        }
        // This computer's keyboard is the one the list shows.
        let host = if cfg!(target_os = "macos") {
            "Cmd+T"
        } else {
            "Ctrl+Shift+T"
        };
        assert_eq!(NEW_TAB.keys(), host);
    }

    #[test]
    fn a_link_opens_with_its_keys_held_and_more_but_not_fewer() {
        let held = match OPEN_LINK.keys_on(Keyboard::HOST) {
            Keys::Click(held) => held,
            other => panic!("{other:?}"),
        };
        assert!(OPEN_LINK.held_by(held));
        assert!(OPEN_LINK.held_by(Modifiers {
            shift: true,
            ..held
        }));
        assert!(!OPEN_LINK.held_by(Modifiers::NONE));
        assert!(!NEW_TAB.held_by(held), "a chord is not held with a click");
    }
}

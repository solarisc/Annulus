//! Every keyboard shortcut (UI-SPEC §7) in one table: the app checks for each one through it,
//! and Settings → Shortcuts lists it, so what the list says is what the keys do. On Windows
//! and Linux, Ctrl with a letter belongs to the shell, so app chords add Shift.

use annulus_ui::kit::{Key, Modifiers, Ui};

/// One shortcut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shortcut {
    /// The group it is listed under.
    pub group: &'static str,
    /// What it does.
    pub action: &'static str,
    /// The chord the app checks for; `None` for one the list only describes (several keys,
    /// or one a widget or the window handles).
    pub chord: Option<(Key, Modifiers)>,
    /// How the list shows one without a chord.
    pub shown: &'static str,
    /// Its name here, so a test can tell the app checks for it.
    pub id: &'static str,
}

const CTRL: Modifiers = Modifiers::CTRL;
const CTRL_SHIFT: Modifiers = Modifiers::CTRL_SHIFT;
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

impl Shortcut {
    const fn chord(
        group: &'static str,
        action: &'static str,
        key: Key,
        modifiers: Modifiers,
        id: &'static str,
    ) -> Self {
        Self {
            group,
            action,
            chord: Some((key, modifiers)),
            shown: "",
            id,
        }
    }

    const fn described(group: &'static str, action: &'static str, shown: &'static str) -> Self {
        Self {
            group,
            action,
            chord: None,
            shown,
            id: "",
        }
    }

    /// Whether its chord was pressed this frame (and takes it).
    pub fn pressed(&self, ui: &mut Ui<'_>) -> bool {
        self.chord
            .is_some_and(|(key, modifiers)| ui.key(key, modifiers))
    }

    /// Whether `key` with `modifiers` is its chord.
    #[must_use]
    pub fn is(&self, key: Key, modifiers: Modifiers) -> bool {
        self.chord == Some((key, modifiers))
    }

    /// The keys as the list shows them: "Ctrl+Shift+T".
    #[must_use]
    pub fn keys(&self) -> String {
        match self.chord {
            Some((key, modifiers)) => chord_text(key, modifiers),
            None => self.shown.to_owned(),
        }
    }
}

/// `key` with `modifiers` written out: "Ctrl+Alt+Shift+B".
#[must_use]
pub fn chord_text(key: Key, modifiers: Modifiers) -> String {
    let mut parts: Vec<String> = Vec::new();
    for (held, name) in [
        (modifiers.ctrl, "Ctrl"),
        (modifiers.alt, "Alt"),
        (modifiers.shift, "Shift"),
        (modifiers.logo, "Win"),
    ] {
        if held {
            parts.push(name.to_owned());
        }
    }
    parts.push(match key {
        Key::Char(c) => c.to_uppercase().to_string(),
        Key::F(n) => format!("F{n}"),
        Key::PageUp => "PageUp".into(),
        Key::PageDown => "PageDown".into(),
        other => format!("{other:?}"),
    });
    parts.join("+")
}

// Tabs.
/// A New Tab page.
pub const NEW_TAB: Shortcut =
    Shortcut::chord("Tabs", "New tab", Key::Char('t'), CTRL_SHIFT, "NEW_TAB");
/// A local terminal with the default shell.
pub const LOCAL: Shortcut = Shortcut::chord(
    "Tabs",
    "Local terminal",
    Key::Char('l'),
    CTRL_SHIFT,
    "LOCAL",
);
/// Close the pane in front, or its tab.
pub const CLOSE: Shortcut = Shortcut::chord(
    "Tabs",
    "Close the pane or tab",
    Key::Char('w'),
    CTRL_SHIFT,
    "CLOSE",
);
/// The next tab.
pub const NEXT_TAB: Shortcut = Shortcut::chord("Tabs", "Next tab", Key::Tab, CTRL, "NEXT_TAB");
/// The tab before.
pub const PREVIOUS_TAB: Shortcut =
    Shortcut::chord("Tabs", "Previous tab", Key::Tab, CTRL_SHIFT, "PREVIOUS_TAB");
/// Go to a tab by its place.
pub const GO_TO_TAB: Shortcut = Shortcut::described("Tabs", "Go to tab 1 to 9", "Alt+1\u{2026}9");
/// Move the tab in front one place left.
pub const MOVE_LEFT: Shortcut = Shortcut::chord(
    "Tabs",
    "Move the tab left",
    Key::PageUp,
    CTRL_SHIFT,
    "MOVE_LEFT",
);
/// Move the tab in front one place right.
pub const MOVE_RIGHT: Shortcut = Shortcut::chord(
    "Tabs",
    "Move the tab right",
    Key::PageDown,
    CTRL_SHIFT,
    "MOVE_RIGHT",
);

// Panes.
/// Split side by side.
pub const SPLIT_RIGHT: Shortcut = Shortcut::chord(
    "Panes",
    "Split to the right",
    Key::Char('d'),
    CTRL_SHIFT,
    "SPLIT_RIGHT",
);
/// Split one above the other.
pub const SPLIT_DOWN: Shortcut = Shortcut::chord(
    "Panes",
    "Split downwards",
    Key::Char('e'),
    CTRL_SHIFT,
    "SPLIT_DOWN",
);
/// Move between panes.
pub const PANE_FOCUS: Shortcut = Shortcut::described("Panes", "Move between panes", "Alt+Arrows");
/// One large pane and the others listed beside it.
pub const FOCUS_MODE: Shortcut = Shortcut::chord(
    "Panes",
    "Focus mode",
    Key::Char('m'),
    CTRL_SHIFT,
    "FOCUS_MODE",
);
/// Type into every pane at once.
pub const BROADCAST: Shortcut = Shortcut::chord(
    "Panes",
    "Type into every pane",
    Key::Char('b'),
    CTRL_SHIFT,
    "BROADCAST",
);
/// Leave the pane in front out of broadcast, or back in.
pub const LEAVE_OUT: Shortcut = Shortcut::chord(
    "Panes",
    "Leave this pane out of typing into every pane",
    Key::Char('b'),
    CTRL_ALT_SHIFT,
    "LEAVE_OUT",
);
/// The snippets beside the terminal.
pub const SNIPPETS: Shortcut = Shortcut::chord(
    "Panes",
    "Snippets beside the terminal",
    Key::Char('.'),
    CTRL_SHIFT,
    "SNIPPETS",
);

// Terminal.
/// Copy the selection.
pub const COPY: Shortcut = Shortcut::chord("Terminal", "Copy", Key::Char('c'), CTRL_SHIFT, "COPY");
/// Paste (the window handles it, and Shift+Insert too).
pub const PASTE: Shortcut = Shortcut::described("Terminal", "Paste", "Ctrl+Shift+V");
/// Find in the output.
pub const FIND: Shortcut = Shortcut::chord("Terminal", "Find", Key::Char('f'), CTRL_SHIFT, "FIND");
/// Clear what scrolled away.
pub const CLEAR: Shortcut = Shortcut::chord(
    "Terminal",
    "Clear scrollback",
    Key::Backspace,
    CTRL_SHIFT,
    "CLEAR",
);
/// Larger text.
pub const ZOOM_IN: Shortcut =
    Shortcut::chord("Terminal", "Larger text", Key::Char('='), CTRL, "ZOOM_IN");
/// Smaller text.
pub const ZOOM_OUT: Shortcut =
    Shortcut::chord("Terminal", "Smaller text", Key::Char('-'), CTRL, "ZOOM_OUT");
/// The text size from Settings.
pub const ZOOM_RESET: Shortcut = Shortcut::chord(
    "Terminal",
    "Text back to 14 pt",
    Key::Char('0'),
    CTRL,
    "ZOOM_RESET",
);

// Hosts.
/// A new host in the vault.
pub const NEW_HOST: Shortcut =
    Shortcut::chord("Hosts", "New host", Key::Char('n'), CTRL_SHIFT, "NEW_HOST");
/// Save the host being edited.
pub const SAVE: Shortcut = Shortcut::chord("Hosts", "Save the host", Key::Char('s'), CTRL, "SAVE");
/// A host's menu (the toolkit opens context menus).
pub const MENU: Shortcut = Shortcut::described("Hosts", "A host's menu", "Shift+F10");
/// Connect to the host chosen.
pub const CONNECT: Shortcut = Shortcut::described("Hosts", "Connect to the host chosen", "Enter");

// App.
/// The command palette.
pub const PALETTE: Shortcut = Shortcut::chord(
    "App",
    "Command palette",
    Key::Char('k'),
    CTRL_SHIFT,
    "PALETTE",
);
/// Settings.
pub const SETTINGS: Shortcut = Shortcut::chord("App", "Settings", Key::Char(','), CTRL, "SETTINGS");
/// Lock the vault.
pub const LOCK: Shortcut =
    Shortcut::chord("App", "Lock the vault", Key::Char('l'), CTRL_ALT, "LOCK");

/// Every shortcut, in the order Settings lists them.
pub const ALL: [Shortcut; 29] = [
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
        let mut seen = std::collections::HashSet::new();
        for shortcut in ALL {
            let Some(chord) = shortcut.chord else {
                assert!(!shortcut.shown.is_empty(), "{}", shortcut.action);
                continue;
            };
            assert!(seen.insert(chord), "{} shares its keys", shortcut.action);
            assert!(
                sources.contains(&format!("shortcuts::{}", shortcut.id)),
                "nothing checks for {} ({})",
                shortcut.action,
                shortcut.keys()
            );
        }
    }

    #[test]
    fn chords_are_written_the_way_the_keyboard_says_them() {
        assert_eq!(NEW_TAB.keys(), "Ctrl+Shift+T");
        assert_eq!(LEAVE_OUT.keys(), "Ctrl+Alt+Shift+B");
        assert_eq!(MOVE_LEFT.keys(), "Ctrl+Shift+PageUp");
        assert_eq!(ZOOM_IN.keys(), "Ctrl+=");
        assert_eq!(CLEAR.keys(), "Ctrl+Shift+Backspace");
        assert_eq!(GO_TO_TAB.keys(), "Alt+1\u{2026}9");
    }
}

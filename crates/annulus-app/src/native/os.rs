//! Server OS icons (F-HOST-009): what each OS key a host stores looks like, the tile cards,
//! rows and tabs draw for a host, and the choices the host editor offers. The OS comes from the
//! server's identification line (`annulus_ssh::os`) or is picked by hand.

use std::hash::{DefaultHasher, Hash, Hasher};

use annulus_config::tokens::{Rgb, TILES};
use annulus_ui::gfx::Canvas as _;
use annulus_ui::gfx::{Align, Color, Rect};
use annulus_ui::icons::{self, IconId};
use annulus_ui::kit::Ui;
use annulus_ui::widgets::{BODY_STRONG, SMALL};
use annulus_vault::hosts::Host;

/// One OS a host can show.
pub struct Os {
    /// The key hosts store.
    pub key: &'static str,
    /// Its name in the editor and for screen readers.
    pub name: &'static str,
    /// Its mark.
    pub icon: IconId,
    /// Its tile.
    pub color: Rgb,
}

/// Every OS, in the order the editor lists them.
pub const ALL: [Os; 12] = [
    Os {
        key: "ubuntu",
        name: "Ubuntu",
        icon: icons::OS_UBUNTU,
        color: Rgb::new(0xC9, 0x4A, 0x1E),
    },
    Os {
        key: "debian",
        name: "Debian",
        icon: icons::OS_DEBIAN,
        color: Rgb::new(0xB0, 0x23, 0x4B),
    },
    Os {
        key: "raspbian",
        name: "Raspberry Pi OS",
        icon: icons::OS_RASPBERRY,
        color: Rgb::new(0xB8, 0x2E, 0x5C),
    },
    Os {
        key: "fedora",
        name: "Fedora",
        icon: icons::OS_FEDORA,
        color: Rgb::new(0x29, 0x5A, 0xA6),
    },
    Os {
        key: "arch",
        name: "Arch Linux",
        icon: icons::OS_ARCH,
        color: Rgb::new(0x17, 0x7C, 0xB5),
    },
    Os {
        key: "alpine",
        name: "Alpine Linux",
        icon: icons::OS_ALPINE,
        color: Rgb::new(0x0E, 0x66, 0x88),
    },
    Os {
        key: "linux",
        name: "Other Linux",
        icon: icons::OS_LINUX,
        color: Rgb::new(0x4A, 0x55, 0x60),
    },
    Os {
        key: "freebsd",
        name: "FreeBSD",
        icon: icons::OS_BSD,
        color: Rgb::new(0x9E, 0x26, 0x26),
    },
    Os {
        key: "netbsd",
        name: "NetBSD",
        icon: icons::OS_BSD,
        color: Rgb::new(0xA8, 0x55, 0x14),
    },
    Os {
        key: "windows",
        name: "Windows",
        icon: icons::OS_WINDOWS,
        color: Rgb::new(0x1D, 0x66, 0xB8),
    },
    Os {
        key: "router",
        name: "Router or device",
        icon: icons::OS_ROUTER,
        color: Rgb::new(0x2F, 0x6E, 0x50),
    },
    Os {
        key: "network",
        name: "Network gear",
        icon: icons::OS_NETWORK,
        color: Rgb::new(0x48, 0x56, 0xA0),
    },
];

/// The OS stored as `key`, if it is one we know.
#[must_use]
pub fn find(key: &str) -> Option<&'static Os> {
    ALL.iter().find(|os| os.key == key)
}

/// The OS `host` shows, if any.
#[must_use]
pub fn of(host: &Host) -> Option<&'static Os> {
    find(&host.os)
}

/// The OS a server's identification line names, as a key to store on its host when it differs
/// from what the host has and was not picked by hand.
#[must_use]
pub fn learned(host: &Host, server_id: &str) -> Option<&'static str> {
    let os = annulus_ssh::os::os_from_server_id(server_id)?;
    (!host.os_by_hand && host.os != os).then_some(os)
}

/// One of the tile hues, stable for `text`: a host's tile when no OS is known, a tag's chip.
#[must_use]
pub fn hue(text: &str) -> Color {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    #[allow(clippy::cast_possible_truncation)] // Only the low bits pick a color.
    let index = (hasher.finish() % TILES.len() as u64) as usize;
    TILES[index].into()
}

/// Up to two letters for a host's tile: the first letters of its first two words, or its
/// first two letters.
#[must_use]
pub fn initials(label: &str) -> String {
    let words: Vec<&str> = label
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();
    let letters: String = match words.as_slice() {
        [] => "?".into(),
        [one] => one.chars().take(2).collect(),
        [first, second, ..] => first
            .chars()
            .take(1)
            .chain(second.chars().take(1))
            .collect(),
    };
    letters.to_uppercase()
}

/// Draws a host's tile into `rect`: its OS's mark when known, else its initials, on a rounded
/// square with a faint top light so it reads as a raised key rather than a flat swatch.
pub fn tile(ui: &mut Ui<'_>, rect: Rect, label: &str, os: Option<&Os>) {
    let fill = os.map_or_else(|| hue(label), |os| os.color.into());
    let round = rect.w * 0.28;
    ui.gfx.fill_rect(rect, round, fill);
    // The top half a little lighter: light from above.
    let top = Rect::new(rect.x, rect.y, rect.w, rect.h * 0.5);
    ui.gfx.push_clip(top);
    ui.gfx.fill_rect(rect, round, Color::WHITE.alpha(0.07));
    ui.gfx.pop_clip();
    ui.gfx
        .stroke_rect(rect.inset(0.5, 0.5), round, 1.0, Color::WHITE.alpha(0.1));
    if let Some(os) = os {
        let size = (rect.w * 0.55).round();
        ui.gfx
            .icon(os.icon, rect.centered(size, size), Color::WHITE);
    } else {
        let font = if rect.w >= 36.0 { BODY_STRONG } else { SMALL };
        ui.gfx
            .text(rect, &initials(label), font, Color::WHITE, Align::Center);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_os_has_a_unique_key_a_mark_and_a_tile_its_mark_shows_on() {
        let keys: std::collections::HashSet<_> = ALL.iter().map(|os| os.key).collect();
        assert_eq!(keys.len(), ALL.len());
        for os in &ALL {
            assert!(icons::icon(os.icon).is_some(), "{}", os.key);
            // A white mark is a graphic: WCAG 1.4.11 asks 3:1.
            assert!(
                Rgb::new(255, 255, 255).contrast(os.color) >= 3.0,
                "{} tile too light",
                os.key
            );
        }
        // Every OS a server can name has a mark.
        for line in [
            "SSH-2.0-OpenSSH_9.6p1 Ubuntu-3ubuntu13.5",
            "SSH-2.0-OpenSSH_9.2p1 Debian-2+deb12u3",
            "SSH-2.0-OpenSSH_7.9p1 Raspbian-10+deb10u2",
            "SSH-2.0-OpenSSH_9.7 FreeBSD-20240806",
            "SSH-2.0-OpenSSH_9.6 NetBSD_Secure_Shell-20231220",
            "SSH-2.0-OpenSSH_for_Windows_9.5",
            "SSH-2.0-dropbear_2022.83",
            "SSH-2.0-Cisco-1.25",
        ] {
            let key = annulus_ssh::os::os_from_server_id(line).unwrap();
            assert!(find(key).is_some(), "{key}");
        }
    }

    #[test]
    fn what_the_server_says_replaces_a_guess_but_never_a_choice() {
        let vault = crate::native::demo::fixture_vault("os-learned", true);
        let mut host = vault.hosts()[0].clone();
        host.os.clear();
        host.os_by_hand = false;
        let ubuntu = "SSH-2.0-OpenSSH_9.6p1 Ubuntu-3ubuntu13.5";
        assert_eq!(learned(&host, ubuntu), Some("ubuntu"));
        host.os = "ubuntu".into();
        assert_eq!(learned(&host, ubuntu), None, "already known");
        assert_eq!(learned(&host, "SSH-2.0-OpenSSH_9.6"), None, "says nothing");
        host.os = "arch".into();
        host.os_by_hand = true;
        assert_eq!(learned(&host, ubuntu), None, "picked by hand");
    }

    #[test]
    fn initials_take_two_words_or_two_letters() {
        assert_eq!(initials("prod-db"), "PD");
        assert_eq!(initials("web"), "WE");
        assert_eq!(initials("cache 1 eu"), "C1");
        assert_eq!(initials("--"), "?");
    }
}

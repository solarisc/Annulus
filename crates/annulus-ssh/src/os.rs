//! What a server's identification line says about its OS (F-HOST-009). Only the line every
//! SSH server sends before key exchange is read; nothing is run on the server to find out.
//! Many servers (Fedora, Arch, RHEL and its rebuilds) name no OS there, and stay unknown.

/// The OS named in `server_id` (e.g. "SSH-2.0-OpenSSH_9.6p1 Ubuntu-3ubuntu13.5"), as one of
/// the keys hosts store: `ubuntu`, `debian`, `raspbian`, `freebsd`, `netbsd`, `windows`,
/// `router` (Dropbear, which routers and small devices run) or `network` (switches).
#[must_use]
pub fn os_from_server_id(server_id: &str) -> Option<&'static str> {
    // A word in the software or comment part, and the OS it means; Raspbian before Debian,
    // whose packages it shares.
    const KNOWN: [(&str, &str); 9] = [
        ("raspbian", "raspbian"),
        ("ubuntu", "ubuntu"),
        ("debian", "debian"),
        ("freebsd", "freebsd"),
        ("netbsd", "netbsd"),
        ("for_windows", "windows"),
        ("dropbear", "router"),
        ("cisco", "network"),
        ("rosssh", "network"),
    ];
    let id = server_id.trim().to_ascii_lowercase();
    // The software and comment after "SSH-2.0-".
    let rest = id.splitn(3, '-').nth(2).unwrap_or(&id);
    KNOWN
        .iter()
        .find(|(word, _)| rest.contains(word))
        .map(|(_, os)| *os)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_os_comes_from_the_servers_identification_line_when_it_names_one() {
        let cases = [
            ("SSH-2.0-OpenSSH_9.6p1 Ubuntu-3ubuntu13.5", Some("ubuntu")),
            ("SSH-2.0-OpenSSH_9.2p1 Debian-2+deb12u3", Some("debian")),
            (
                "SSH-2.0-OpenSSH_7.9p1 Raspbian-10+deb10u2",
                Some("raspbian"),
            ),
            ("SSH-2.0-OpenSSH_9.7 FreeBSD-20240806", Some("freebsd")),
            (
                "SSH-2.0-OpenSSH_9.6 NetBSD_Secure_Shell-20231220-hpn13v14-lpk",
                Some("netbsd"),
            ),
            ("SSH-2.0-OpenSSH_for_Windows_9.5", Some("windows")),
            ("SSH-2.0-dropbear_2022.83", Some("router")),
            ("SSH-2.0-Cisco-1.25", Some("network")),
            ("SSH-2.0-ROSSSH", Some("network")),
            // Fedora, Arch and RHEL say only which OpenSSH they run.
            ("SSH-2.0-OpenSSH_9.6", None),
            ("SSH-2.0-OpenSSH_8.7", None),
            ("", None),
        ];
        for (line, os) in cases {
            assert_eq!(os_from_server_id(line), os, "{line:?}");
        }
    }
}

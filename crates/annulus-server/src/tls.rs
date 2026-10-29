//! The server's own certificate, for a server on a home network with no certificate from a
//! public authority (ADR-0018): made once with `annulus-server tls-init`, and trusted by the
//! app after the user compares its fingerprint with what `annulus-server fingerprint` prints.

use std::io::Write as _;
use std::path::Path;

use base64::Engine as _;

/// A new self-signed certificate for `names` (host names or IP addresses the app will use),
/// as PEM: the certificate, and its private key.
///
/// # Errors
/// A name is not usable in a certificate.
pub fn self_signed(names: &[String]) -> Result<(String, String), String> {
    let made = rcgen::generate_simple_self_signed(names.to_vec())
        .map_err(|error| format!("cannot make the certificate: {error}"))?;
    Ok((made.cert.pem(), made.signing_key.serialize_pem()))
}

/// Writes a new certificate for `names` into `dir` as `cert.pem` and `key.pem` (the key
/// readable only by its owner on Unix); never over existing files. Returns the certificate's
/// fingerprint.
///
/// # Errors
/// A name is not usable, a file exists already, or writing failed.
pub fn init(dir: &Path, names: &[String]) -> Result<String, String> {
    let (cert, key) = self_signed(names)?;
    std::fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    let write = |name: &str, text: &str, private: bool| -> Result<(), String> {
        let path = dir.join(name);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        if private {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        #[cfg(not(unix))]
        let _ = private;
        options
            .open(&path)
            .and_then(|mut file| file.write_all(text.as_bytes()))
            .map_err(|error| format!("{}: {error}", path.display()))
    };
    // Neither may exist yet, so a refused run never leaves a new key beside an old
    // certificate.
    for name in ["cert.pem", "key.pem"] {
        if dir.join(name).exists() {
            return Err(format!("{} exists already", dir.join(name).display()));
        }
    }
    write("key.pem", &key, true)?;
    write("cert.pem", &cert, false)?;
    fingerprint_of_pem(&cert)
}

/// The fingerprint of the last certificate in `pem`: the one the app pins (the server's own
/// self-signed certificate, or the top of its chain).
///
/// # Errors
/// `pem` holds no certificate.
pub fn fingerprint_of_pem(pem: &str) -> Result<String, String> {
    const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
    const END: &str = "-----END CERTIFICATE-----";
    let last = pem
        .split(BEGIN)
        .skip(1)
        .filter_map(|block| block.split(END).next())
        .last()
        .ok_or("no certificate in the file")?;
    let text: String = last.chars().filter(|c| !c.is_whitespace()).collect();
    let der = base64::engine::general_purpose::STANDARD
        .decode(text)
        .map_err(|_| "the certificate is not valid PEM")?;
    Ok(annulus_proto::fingerprint(&der))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_certificate_is_written_once_with_a_private_key_and_its_fingerprint() {
        let dir = std::env::temp_dir().join(format!("annulus-tls-init-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let names = vec!["192.0.2.20".to_owned(), "sync.lan".to_owned()];
        let printed = init(&dir, &names).unwrap();
        let cert = std::fs::read_to_string(dir.join("cert.pem")).unwrap();
        assert_eq!(fingerprint_of_pem(&cert).unwrap(), printed);
        assert_eq!(printed.len(), 32 * 3 - 1, "{printed}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(dir.join("key.pem"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert!(init(&dir, &names).is_err(), "never over existing files");
        std::fs::remove_file(dir.join("key.pem")).unwrap();
        assert!(
            init(&dir, &names).is_err(),
            "no new key beside an old certificate"
        );
        assert!(!dir.join("key.pem").exists());
        assert!(fingerprint_of_pem("not a certificate").is_err());
    }
}

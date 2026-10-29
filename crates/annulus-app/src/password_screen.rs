//! Changing the master password in Settings (F-SYNC-009). The Argon2id work runs on a
//! worker thread. When the vault is signed in to an account, the account is updated first
//! (other devices are signed out and sign in again with the new password), and only then
//! the local vault, so the two never disagree.

/// Shortest master password accepted (as at setup).
const MIN_PASSWORD: usize = 10;

/// Why the new password cannot be used, if it cannot.
pub(crate) fn problem(new: &str, confirm: &str) -> Option<&'static str> {
    if new.chars().count() < MIN_PASSWORD {
        Some("Use at least 10 characters.")
    } else if !annulus_vault::strength::estimate(new).is_strong() {
        Some("Too easy to guess. Make it longer or less common.")
    } else if new != confirm {
        Some("The two new passwords differ.")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_password_must_be_long_strong_and_typed_twice() {
        assert!(problem("short", "short").is_some());
        assert_eq!(
            problem("qwerty123456", "qwerty123456"),
            Some("Too easy to guess. Make it longer or less common.")
        );
        assert!(problem("long enough pw", "long enough pX").is_some());
        assert_eq!(problem("long enough pw", "long enough pw"), None);
    }
}

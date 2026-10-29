//! How hard a master password is to guess, by zxcvbn: the password is
//! matched against what a guesser tries first (about 30,000 common passwords, English words,
//! first names and surnames, also reversed or in l33t, keyboard patterns, repeats, sequences,
//! years and dates), and the number of guesses is that of the cheapest way to build it.
//!
//! The result decides whether a new master password is accepted: with a stolen server
//! database an attacker can guess offline, at one Argon2id run per guess.

/// The estimate for one password.
#[derive(Clone, Debug, PartialEq)]
pub struct Strength {
    /// log10 of the guesses an attacker needs.
    pub guesses_log10: f64,
    /// 0 (guessed at once) to 4 (strong), zxcvbn's score.
    pub score: u8,
    /// What makes it easy to guess, while it is not strong.
    pub hint: Option<String>,
}

impl Strength {
    /// Strong enough for a master password: 10^10 guesses or more, which is months of work
    /// for a GPU against the vault's Argon2id.
    #[must_use]
    pub fn is_strong(&self) -> bool {
        self.score >= 4
    }

    /// A word for the score.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self.score {
            0 | 1 => "Weak",
            2 => "Fair",
            3 => "Almost there",
            _ => "Strong",
        }
    }
}

/// The most characters read: zxcvbn's work grows quickly with length, and a longer password
/// is only stronger.
const LONGEST: usize = 100;

/// Words a guesser would try first against this app.
const OURS: [&str; 3] = ["annulus", "vault", "terminal"];

/// Estimates `password` (its first [`LONGEST`] characters).
#[must_use]
pub fn estimate(password: &str) -> Strength {
    let read: String = password.chars().take(LONGEST).collect();
    let entropy = zxcvbn::zxcvbn(&read, &OURS);
    let score = u8::from(entropy.score());
    let hint = (score < 4).then(|| {
        entropy
            .feedback()
            .and_then(|feedback| {
                feedback
                    .warning()
                    .map(|warning| warning.to_string())
                    .or_else(|| feedback.suggestions().first().map(ToString::to_string))
            })
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| "Add another word or two. Uncommon words are better.".into())
    });
    Strength {
        guesses_log10: entropy.guesses_log10(),
        score,
        hint,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_passwords_words_and_their_disguises_are_weak() {
        for password in [
            "password",
            "Password1",
            "p@ssw0rd",
            "P@$$w0rd!",
            "qwerty123",
            "iloveyou",
            "letmein!",
            "drowssap",
            "Dragon2024",
            "Summer2024!!",
            "princessmonkey",
            "annulus123",
            "xylophones",
            "Elizabeth1985",
            "Washington1",
        ] {
            let strength = estimate(password);
            assert!(!strength.is_strong(), "{password}: {strength:?}");
            assert!(strength.hint.is_some(), "{password} needs a hint");
        }
    }

    #[test]
    fn repeats_sequences_rows_and_dates_are_seen_through() {
        for password in [
            "aaaaaaaaaaaa",
            "abcabcabcabc",
            "abcdefghijklm",
            "9876543210zyx",
            "asdfghjkl;'",
            "zxcvbnm,./",
            "01011990",
            "1990-01-01",
        ] {
            let strength = estimate(password);
            assert!(!strength.is_strong(), "{password}: {strength:?}");
            assert!(strength.hint.is_some(), "{password}: {strength:?}");
        }
    }

    #[test]
    fn random_text_and_passphrases_are_strong() {
        for password in [
            "kx8#vQ2m!zTb",
            "correct horse battery staple",
            "violet-anchor-pebble-tundra",
            "gS7!pq0Lw2#Z",
        ] {
            let strength = estimate(password);
            assert!(strength.is_strong(), "{password}: {strength:?}");
            assert_eq!(strength.hint, None);
        }
    }

    #[test]
    fn empty_unicode_and_huge_input_are_estimated_at_once() {
        assert_eq!(estimate("").score, 0);
        let started = std::time::Instant::now();
        for password in [
            "xQ7!".repeat(50_000),
            "e\u{301}".repeat(10_000),
            "\u{5bc6}\u{7801}".repeat(4),
        ] {
            let _ = estimate(&password);
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        assert!(
            !estimate(&"xQ7!".repeat(50_000)).is_strong(),
            "one piece repeated"
        );
    }
}

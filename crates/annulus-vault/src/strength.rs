//! How hard a master password is to guess, estimated the way zxcvbn
//! does it: the password is split into the patterns a guesser tries first (common passwords
//! and words, also reversed or in l33t, repeats, sequences, rows of keys, years and dates) and
//! random characters, and the number of guesses is that of the cheapest split.
//!
//! The result decides whether a new master password is accepted: with a stolen server
//! database an attacker can guess offline, at one Argon2id run per guess.

use std::collections::HashMap;
use std::sync::LazyLock;

/// The estimate for one password.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Strength {
    /// log10 of the guesses an attacker needs.
    pub guesses_log10: f64,
    /// 0 (guessed at once) to 4 (strong), on zxcvbn's scale.
    pub score: u8,
    /// What makes it easy to guess, while it is not strong.
    pub hint: Option<&'static str>,
}

impl Strength {
    /// Strong enough for a master password: 10^10 guesses, which is months of work for a
    /// GPU against the vault's Argon2id.
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

/// Estimates `password`. Only its first [`LONGEST`] characters are read; that is plenty.
#[must_use]
pub fn estimate(password: &str) -> Strength {
    let chars: Vec<char> = password.chars().take(LONGEST).collect();
    let (guesses, pattern) = cheapest(&chars);
    let guesses_log10 = guesses.max(1.0).log10();
    let score = match guesses_log10 {
        log if log < 3.0 => 0,
        log if log < 6.0 => 1,
        log if log < 8.0 => 2,
        log if log < 10.0 => 3,
        _ => 4,
    };
    let hint = (score < 4).then_some(match pattern {
        Some(Pattern::Top) => "This is one of the most used passwords.",
        Some(Pattern::Word) => "Common passwords, words and names are easy to guess.",
        Some(Pattern::Repeat) => {
            "Repeats like \u{201c}aaa\u{201d} or \u{201c}abcabc\u{201d} are easy to guess."
        }
        Some(Pattern::Sequence) => {
            "Sequences like \u{201c}abc\u{201d} or \u{201c}4321\u{201d} are easy to guess."
        }
        Some(Pattern::Keyboard) => "Rows of keys like \u{201c}qwerty\u{201d} are easy to guess.",
        Some(Pattern::Date) => "Dates and years are easy to guess.",
        None => "Add another word or two. Uncommon words are better.",
    });
    Strength {
        guesses_log10,
        score,
        hint,
    }
}

/// The most characters read.
const LONGEST: usize = 100;
/// Guesses for one random character (zxcvbn's brute-force cardinality).
const PER_CHARACTER: f64 = 10.0;
/// The year dates are measured from.
const THIS_YEAR: i32 = 2026;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pattern {
    /// One of the few most used passwords.
    Top,
    Word,
    Repeat,
    Sequence,
    Keyboard,
    Date,
}

/// A pattern found at `start..end`.
struct Found {
    start: usize,
    end: usize,
    guesses: f64,
    pattern: Pattern,
}

/// The fewest guesses for a start of the password in some number of pieces, and the longest
/// pattern used (its length and kind).
type Best = Option<(f64, Option<(usize, Pattern)>)>;

/// The guesses for `chars` and the longest pattern of the cheapest split.
fn cheapest(chars: &[char]) -> (f64, Option<Pattern>) {
    let n = chars.len();
    if n == 0 {
        return (1.0, None);
    }
    let mut found = Vec::new();
    words(chars, &mut found);
    repeats(chars, &mut found);
    sequences(chars, &mut found);
    keyboard(chars, &mut found);
    dates(chars, &mut found);
    let mut ending: Vec<Vec<Found>> = (0..=n).map(|_| Vec::new()).collect();
    for mut piece in found {
        // A pattern inside a longer password still costs a guesser some work.
        if piece.end - piece.start < n {
            piece.guesses = piece.guesses.max(if piece.end - piece.start == 1 {
                10.0
            } else {
                50.0
            });
        }
        ending[piece.end].push(piece);
    }
    // best[end][count]: the fewest guesses for chars[..end] in `count` pieces.
    let mut best: Vec<Vec<Best>> = vec![vec![None; n + 1]; n + 1];
    best[0][0] = Some((1.0, None));
    for end in 1..=n {
        for count in 1..=end {
            let mut lowest: Best = None;
            let mut take = |guesses: f64, longest: Option<(usize, Pattern)>| {
                if lowest.is_none_or(|(low, _)| guesses < low) {
                    lowest = Some((guesses, longest));
                }
            };
            for (start, before) in best.iter().enumerate().take(end) {
                if let Some((before, longest)) = before[count - 1] {
                    let random = PER_CHARACTER.powi(power(end - start));
                    take(before * random, longest);
                }
            }
            for piece in &ending[end] {
                if let Some((before, longest)) = best[piece.start][count - 1] {
                    let length = piece.end - piece.start;
                    let longest = match longest {
                        Some((other, _)) if other >= length => longest,
                        _ => Some((length, piece.pattern)),
                    };
                    take(before * piece.guesses, longest);
                }
            }
            best[end][count] = lowest;
        }
    }
    let mut total = f64::INFINITY;
    let mut pattern = None;
    let mut ways = 1.0;
    for (count, whole) in best[n].iter().enumerate().skip(1) {
        ways *= float(count);
        if let Some((product, longest)) = *whole {
            // More pieces cost more: the guesser has to try the orders, and zxcvbn's floor
            // for every extra piece.
            let guesses = ways * product + 10_000_f64.powi(power(count - 1));
            if guesses < total {
                total = guesses;
                pattern = longest.map(|(_, pattern)| pattern);
            }
        }
    }
    (total, pattern)
}

/// Common passwords and words, also reversed and in l33t.
fn words(chars: &[char], found: &mut Vec<Found>) {
    let lower: Vec<char> = chars.iter().map(|c| lowercase(*c)).collect();
    for start in 0..chars.len() {
        for end in start + 3..=chars.len().min(start + *LONGEST_WORD) {
            for (plain, changed) in unleet(&lower[start..end]) {
                let forward: String = plain.iter().collect();
                let backward: String = plain.iter().rev().collect();
                for (text, reversed) in [(forward, false), (backward, true)] {
                    let Some(&rank) = RANKS.get(text.as_str()) else {
                        continue;
                    };
                    let mut guesses = float(rank) * case_variants(&chars[start..end]);
                    if changed > 0 {
                        guesses *= 2_f64.powi(power(changed));
                    }
                    if reversed {
                        guesses *= 2.0;
                    }
                    let pattern = if rank <= 20 && changed == 0 && !reversed {
                        Pattern::Top
                    } else {
                        Pattern::Word
                    };
                    found.push(Found {
                        start,
                        end,
                        guesses,
                        pattern,
                    });
                }
            }
        }
    }
}

/// `piece` as typed, then with l33t undone (once reading `1` as `i`, once as `l`), with
/// how many characters were changed.
fn unleet(piece: &[char]) -> Vec<(Vec<char>, usize)> {
    let mut out = vec![(piece.to_vec(), 0)];
    for one in ['i', 'l'] {
        let mut changed = 0;
        let plain: Vec<char> = piece
            .iter()
            .map(|c| {
                let letter = match c {
                    '4' | '@' => 'a',
                    '3' => 'e',
                    '1' => one,
                    '!' | '|' => 'i',
                    '0' => 'o',
                    '$' | '5' => 's',
                    '7' | '+' => 't',
                    other => *other,
                };
                if letter != *c {
                    changed += 1;
                }
                letter
            })
            .collect();
        if changed > 0 && !out.iter().any(|(seen, _)| *seen == plain) {
            out.push((plain, changed));
        }
    }
    out
}

/// How many ways a guesser tries the capital letters of a word: none, the first or last, all,
/// or (rarely) a mix.
fn case_variants(piece: &[char]) -> f64 {
    let upper = piece.iter().filter(|c| c.is_uppercase()).count();
    let lower = piece.iter().filter(|c| c.is_lowercase()).count();
    if upper == 0 {
        return 1.0;
    }
    let first = piece.first().is_some_and(|c| c.is_uppercase());
    let last = piece.last().is_some_and(|c| c.is_uppercase());
    if lower == 0 || (upper == 1 && (first || last)) {
        return 2.0;
    }
    (1..=upper.min(lower))
        .map(|chosen| choose(upper + lower, chosen))
        .sum()
}

/// One piece repeated: `aaa`, `abab`, `abcabcabc`.
fn repeats(chars: &[char], found: &mut Vec<Found>) {
    let n = chars.len();
    for start in 0..n {
        for period in 1..=((n - start) / 2).min(8) {
            let mut end = start + period;
            while end < n && chars[end] == chars[end - period] {
                end += 1;
            }
            let times = (end - start) / period;
            if times >= 2 && times * period >= 3 {
                let (base, _) = cheapest(&chars[start..start + period]);
                found.push(Found {
                    start,
                    end: start + times * period,
                    guesses: base * float(times),
                    pattern: Pattern::Repeat,
                });
            }
        }
    }
}

/// Runs like `abcd`, `9876` or `KLMN`.
fn sequences(chars: &[char], found: &mut Vec<Found>) {
    let n = chars.len();
    let mut start = 0;
    while start + 2 < n {
        let Some(delta) = step(chars[start], chars[start + 1]) else {
            start += 1;
            continue;
        };
        let mut end = start + 2;
        while end < n && step(chars[end - 1], chars[end]) == Some(delta) {
            end += 1;
        }
        if end - start >= 3 {
            let first = chars[start];
            let base = if "aAzZ019".contains(first) {
                4.0
            } else if first.is_ascii_digit() {
                10.0
            } else {
                26.0
            };
            let direction = if delta < 0 { 2.0 } else { 1.0 };
            found.push(Found {
                start,
                end,
                guesses: base * float(end - start) * direction,
                pattern: Pattern::Sequence,
            });
            start = end - 1;
        } else {
            start += 1;
        }
    }
}

/// +1 or -1 when `b` follows `a` in the alphabet or the digits.
fn step(a: char, b: char) -> Option<i32> {
    let same = (a.is_ascii_lowercase() && b.is_ascii_lowercase())
        || (a.is_ascii_uppercase() && b.is_ascii_uppercase())
        || (a.is_ascii_digit() && b.is_ascii_digit());
    let delta = i32::try_from(u32::from(b)).ok()? - i32::try_from(u32::from(a)).ok()?;
    (same && delta.abs() == 1).then_some(delta)
}

/// Four or more neighbouring keys of one keyboard row, either way: `qwer`, `lkjh`, `7890`.
fn keyboard(chars: &[char], found: &mut Vec<Found>) {
    const ROWS: [&str; 4] = [
        "`1234567890-=",
        "qwertyuiop[]\\",
        "asdfghjkl;'",
        "zxcvbnm,./",
    ];
    let lower: Vec<char> = chars.iter().map(|c| lowercase(*c)).collect();
    for row in ROWS {
        let keys: Vec<char> = row.chars().collect();
        for direction in [1_isize, -1] {
            for start in 0..lower.len() {
                let Some(mut at) = keys.iter().position(|key| *key == lower[start]) else {
                    continue;
                };
                let mut end = start + 1;
                while end < lower.len() {
                    let Some(next) = at.checked_add_signed(direction) else {
                        break;
                    };
                    if keys.get(next) != Some(&lower[end]) {
                        break;
                    }
                    at = next;
                    end += 1;
                }
                if end - start >= 4 {
                    found.push(Found {
                        start,
                        end,
                        guesses: 94.0 * float(end - start),
                        pattern: Pattern::Keyboard,
                    });
                }
            }
        }
    }
}

/// Years from 1900 to 2049, and dates with or without separators (`01011990`, `1990-1-1`).
fn dates(chars: &[char], found: &mut Vec<Found>) {
    let n = chars.len();
    for start in 0..n {
        if let Some(year) = number(chars.get(start..start + 4))
            && (1900..=2049).contains(&year)
        {
            found.push(Found {
                start,
                end: start + 4,
                guesses: years_from_now(year),
                pattern: Pattern::Date,
            });
        }
        for length in 4..=10 {
            let Some(piece) = chars.get(start..start + length) else {
                break;
            };
            if let Some((year, separated)) = date(piece) {
                let factor = if separated { 4.0 } else { 1.0 };
                found.push(Found {
                    start,
                    end: start + length,
                    guesses: 365.0 * years_from_now(year) * factor,
                    pattern: Pattern::Date,
                });
            }
        }
    }
}

/// The year of `piece` read as a day, month and year in any usual order, and whether it
/// had separators.
fn date(piece: &[char]) -> Option<(i32, bool)> {
    let separator = piece.iter().find(|c| !c.is_ascii_digit()).copied();
    let parts: Vec<&[char]> = match separator {
        Some(separator) if "/-._ ".contains(separator) => {
            let parts: Vec<&[char]> = piece.split(|c| *c == separator).collect();
            if parts.len() != 3 || parts.iter().any(|part| part.is_empty() || part.len() > 4) {
                return None;
            }
            parts
        }
        Some(_) => return None,
        None => match piece.len() {
            6 => vec![&piece[..2], &piece[2..4], &piece[4..]],
            8 => {
                // Day, month, year or year, month, day.
                if let Some(year) = date_parts(&piece[..2], &piece[2..4], &piece[4..]) {
                    return Some((year, false));
                }
                vec![&piece[..4], &piece[4..6], &piece[6..]]
            }
            _ => return None,
        },
    };
    let year = date_parts(parts[0], parts[1], parts[2])
        .or_else(|| date_parts(parts[2], parts[1], parts[0]))?;
    Some((year, separator.is_some()))
}

/// The year when `a`, `b` are a day and a month (either order) and `year` a year.
fn date_parts(a: &[char], b: &[char], year: &[char]) -> Option<i32> {
    let (a, b, year) = (number(Some(a))?, number(Some(b))?, number(Some(year))?);
    let year = match year {
        0..=49 => 2000 + year,
        50..=99 => 1900 + year,
        1900..=2049 => year,
        _ => return None,
    };
    let day_month = |day: i32, month: i32| (1..=31).contains(&day) && (1..=12).contains(&month);
    (day_month(a, b) || day_month(b, a)).then_some(year)
}

/// `digits` as a number, when they are all digits.
fn number(digits: Option<&[char]>) -> Option<i32> {
    let digits = digits?;
    if digits.is_empty() || digits.len() > 4 || !digits.iter().all(char::is_ascii_digit) {
        return None;
    }
    digits.iter().collect::<String>().parse().ok()
}

fn years_from_now(year: i32) -> f64 {
    f64::from((year - THIS_YEAR).abs().max(20))
}

fn lowercase(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

fn float(n: usize) -> f64 {
    f64::from(u32::try_from(n).unwrap_or(u32::MAX))
}

fn power(n: usize) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

fn choose(n: usize, k: usize) -> f64 {
    (0..k).fold(1.0, |product, i| product * float(n - i) / float(i + 1))
}

/// The rank of each listed word (1 is the most used).
static RANKS: LazyLock<HashMap<&'static str, usize>> = LazyLock::new(|| {
    let mut ranks = HashMap::new();
    for (index, word) in COMMON.split_whitespace().enumerate() {
        ranks.entry(word).or_insert(index + 1);
    }
    ranks
});

static LONGEST_WORD: LazyLock<usize> = LazyLock::new(|| {
    COMMON
        .split_whitespace()
        .map(|word| word.chars().count())
        .max()
        .unwrap_or(0)
});

/// Widely used passwords first, then common words and names, most used first.
const COMMON: &str = "
password 123456 123456789 qwerty 12345678 111111 1234567890 1234567 12345 123123 000000
iloveyou abc123 password1 qwerty123 1q2w3e4r admin letmein welcome monkey dragon 654321
sunshine master 666666 qwertyuiop 123321 mustang 1qaz2wsx 7777777 121212 football baseball
shadow superman michael princess starwars login charlie aa123456 donald qazwsx trustno1
whatever freedom hello ninja azerty loveme flower hottie jordan jennifer hunter buster soccer
harley batman andrew tigger killer george pepper ginger joshua cheese amanda summer ashley
nicole chelsea biteme matthew access yankees dallas austin thunder taylor matrix minecraft
pokemon computer internet samsung google apple secret changeme default guest test user root
toor administrator qwerty1 zxcvbnm asdfgh asdfghjkl password123 welcome1 hello123 admin123
letmein1 test123 pass123 passwd mypass mypassword private nothing iloveu love lovely angel
baby babygirl family friends forever jesus blessed happy smile cookie chocolate butterfly
purple orange yellow silver golden black white green blue pink diamond money rainbow phoenix
tiger lion eagle wolf bear dog cat puppy kitty bubbles maggie daisy lucky buddy rocky bailey
sophie molly chloe jessica daniel david robert thomas william james john richard joseph mark
paul steven kevin brian edward anthony chris mike alex peter sarah emily hannah emma olivia
lauren michelle melissa natalie rachel anna maria sophia monday tuesday wednesday thursday
friday saturday sunday january february march april june july august september october
november december spring winter autumn school college music guitar player gamer games hockey
golf tennis boston chicago london paris berlin newyork texas florida california canada
america england germany france india china japan mexico annulus terminal server
linux windows ubuntu debian sftp pass qwer asdf zxcv abcd abcdef abcdefg temp superstar star
starlight moon sky ocean beach island mountain river forest fire water earth wind storm snow
rain dream magic wizard knight king queen prince lady boss chief captain hero legend samurai
warrior soldier sniper hacker coffee beer pizza banana cherry lemon peach strawberry chicken
bacon mother father sister brother friend girl boy man woman home house work office company
master123 qwe asd zxc abc letme mylove sweet sweety honey darling forget ever never always
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_passwords_and_their_disguises_are_weak() {
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
        ] {
            let strength = estimate(password);
            assert!(!strength.is_strong(), "{password}: {strength:?}");
            assert!(strength.hint.is_some(), "{password} needs a hint");
        }
        assert_eq!(
            estimate("password").hint,
            Some("This is one of the most used passwords.")
        );
    }

    #[test]
    fn repeats_sequences_rows_and_dates_are_seen_through() {
        for (password, hint) in [
            ("aaaaaaaaaaaa", "Repeats"),
            ("abcabcabcabc", "Repeats"),
            ("abcdefghijklm", "Sequences"),
            ("9876543210zyx", "Sequences"),
            ("asdfghjkl;'", "Rows"),
            ("zxcvbnm,./", "Rows"),
            ("01011990", "Dates"),
            ("1990-01-01", "Dates"),
        ] {
            let strength = estimate(password);
            assert!(!strength.is_strong(), "{password}: {strength:?}");
            assert!(
                strength.hint.is_some_and(|text| text.starts_with(hint)),
                "{password}: {strength:?}"
            );
        }
    }

    #[test]
    fn random_text_and_passphrases_are_strong() {
        for password in [
            "kx8#vQ2m!zTb",
            "Tr0ub4dor&3x",
            "correct horse battery staple",
            "violet-anchor-pebble-tundra",
            "MyDog$Rex2019!",
        ] {
            let strength = estimate(password);
            assert!(strength.is_strong(), "{password}: {strength:?}");
            assert_eq!(strength.hint, None);
        }
    }

    #[test]
    fn a_huge_paste_is_estimated_at_once() {
        let started = std::time::Instant::now();
        let strength = estimate(&"xQ7!".repeat(50_000));
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        assert!(!strength.is_strong(), "one piece repeated: {strength:?}");
    }
}

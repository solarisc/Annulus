//! The Logs screen (F-LOG-002…004): saved session logs, newest first, filtered by title;
//! one opens read-only with search, saves as a text file, or is deleted.

use std::path::PathBuf;

use annulus_vault::logs::SessionLog;

/// `YYYY-MM-DD HH:MM` (UTC) for seconds since the Unix epoch.
pub(crate) fn date(seconds: u64) -> String {
    let days = i64::try_from(seconds / 86_400).unwrap_or(0);
    let minutes = seconds % 86_400 / 60;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}",
        minutes / 60,
        minutes % 60
    )
}

/// A file name for a log: its title and date, with characters files cannot have replaced.
fn file_name(log: &SessionLog) -> String {
    let title: String = log
        .title
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!(
        "{title}-{}.txt",
        date(log.saved_at).replace([' ', ':'], "-")
    )
}

/// The user's Downloads folder, else home, else the temp folder.
pub(crate) fn downloads() -> PathBuf {
    directories::UserDirs::new()
        .and_then(|dirs| {
            dirs.download_dir()
                .map(std::path::Path::to_path_buf)
                .or_else(|| Some(dirs.home_dir().to_path_buf()))
        })
        .unwrap_or_else(std::env::temp_dir)
}

/// Writes `log` to `dir` under a free name (never over an existing file).
pub(crate) fn export(log: &SessionLog, dir: &std::path::Path) -> std::io::Result<PathBuf> {
    let name = file_name(log);
    let mut path = dir.join(&name);
    let mut n = 1;
    while path.exists() {
        path = dir.join(name.replace(".txt", &format!(" ({n}).txt")));
        n += 1;
    }
    std::fs::write(&path, &log.text)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exports_get_a_safe_unique_name_and_never_overwrite() {
        let dir = std::env::temp_dir().join(format!("annulus-logs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = SessionLog {
            id: 1,
            title: "admin@db:22 / prod".into(),
            saved_at: 0,
            text: "hello\n".into(),
        };
        let first = export(&log, &dir).unwrap();
        let second = export(&log, &dir).unwrap();
        assert_eq!(
            first.file_name().unwrap(),
            "admin_db_22___prod-1970-01-01-00-00.txt"
        );
        assert_ne!(first, second);
        assert_eq!(std::fs::read_to_string(second).unwrap(), "hello\n");
    }
}

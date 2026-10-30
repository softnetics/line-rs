//! `results.md` export.

use std::time::{SystemTime, UNIX_EPOCH};

pub struct ReportRow<'a> {
    pub id: &'a str,
    pub auto: &'a str,
    pub duration_ms: Option<u64>,
    pub verdict: Option<&'a str>,
    pub notes: &'a str,
    pub error: Option<&'a str>,
}

/// Make text safe inside one markdown table cell.
pub fn cell(s: &str) -> String {
    let flat = s.replace("\r\n", " ").replace(['\n', '\r'], " ").replace('|', "\\|");
    let flat = flat.trim();
    if flat.chars().count() > 300 {
        let cut: String = flat.chars().take(300).collect();
        format!("{cut}…")
    } else {
        flat.to_string()
    }
}

/// `YYYY-MM-DD HH:MM UTC` from unix seconds (no date crate).
pub fn utc_datetime(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02} UTC", rem / 3600, (rem % 3600) / 60)
}

/// Short git commit of the working tree, if available.
pub fn git_commit() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".into())
}

pub fn now_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

pub fn render(date: &str, commit: &str, device: &str, rows: &[ReportRow<'_>]) -> String {
    let mut out = String::new();
    out.push_str("# line-rs live test results\n\n");
    out.push_str(&format!("Date: {date}  \nCommit: {}  \nDevice: {}\n\n", cell(commit), cell(device)));
    out.push_str("| test | auto result | manual verdict | notes | error |\n|---|---|---|---|---|\n");
    for r in rows {
        let auto = match r.duration_ms {
            Some(ms) => format!("{} ({:.1}s)", r.auto, ms as f64 / 1000.0),
            None => r.auto.to_string(),
        };
        let verdict = match r.verdict {
            Some("ok") => "ถูกต้อง (ok)",
            Some("bad") => "ไม่ถูกต้อง (bad)",
            _ => "",
        };
        out.push_str(&format!(
            "| `{}` | {} | {} | {} | {} |\n",
            r.id,
            cell(&auto),
            verdict,
            cell(r.notes),
            cell(r.error.unwrap_or(""))
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(utc_datetime(0), "1970-01-01 00:00 UTC");
        assert_eq!(utc_datetime(951_782_400), "2000-02-29 00:00 UTC");
        assert_eq!(utc_datetime(1_792_540_800 + 3_660), "2026-10-21 01:01 UTC");
    }

    #[test]
    fn cells_are_escaped() {
        assert_eq!(cell("a|b\nc"), "a\\|b c");
        assert_eq!(cell(&"x".repeat(400)).chars().count(), 301);
    }

    #[test]
    fn renders_table() {
        let rows = [
            ReportRow {
                id: "write::send_text",
                auto: "pass",
                duration_ms: Some(5200),
                verdict: Some("ok"),
                notes: "seen in Keep Memo",
                error: None,
            },
            ReportRow {
                id: "read_only::keep_size",
                auto: "fail",
                duration_ms: Some(100),
                verdict: None,
                notes: "",
                error: Some("HTTP 404 | not found"),
            },
            ReportRow { id: "events::unsend", auto: "idle", duration_ms: None, verdict: None, notes: "", error: None },
        ];
        let md = render("2026-10-21 01:01 UTC", "abc1234", "DESKTOPWIN", &rows);
        assert!(md.contains("Commit: abc1234"));
        assert!(md.contains("Device: DESKTOPWIN"));
        assert!(md.contains("| test | auto result | manual verdict | notes | error |"));
        assert!(md.contains("| `write::send_text` | pass (5.2s) | ถูกต้อง (ok) | seen in Keep Memo |  |"));
        assert!(md.contains("| `read_only::keep_size` | fail (0.1s) |  |  | HTTP 404 \\| not found |"));
        assert!(md.contains("| `events::unsend` | idle |  |  |  |"));
    }
}

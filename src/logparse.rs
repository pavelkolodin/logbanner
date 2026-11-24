//! Разбор сырых логов nginx и извлечение HTTP-`path` из строк.
//!
//! Формат строки — стандартный nginx combined, например:
//!
//! ```text
//! 94.154.43.229 - - [02/Aug/2026:02:09:30 +0000] "GET / HTTP/1.1" 200 1545 "-" "UA"
//! ```
//!
//! Request-блок (`"GET /path HTTP/1.1"`) — первая пара двойных кавычек в строке.

use anyhow::{Context, Result};
use std::collections::HashSet;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

/// Статистика разбора файла логов.
#[derive(Debug, Default)]
pub struct ParseStats {
    /// Всего строк в файле.
    pub lines_total: usize,
    /// Строк, из которых удалось извлечь path.
    pub lines_with_path: usize,
}

/// Читает файл логов и возвращает множество уникальных path + статистику.
pub fn extract_unique_paths(path: &Path) -> Result<(HashSet<String>, ParseStats)> {
    let file = File::open(path)
        .with_context(|| format!("cannot open log file {}", path.display()))?;
    let reader = BufReader::new(file);

    let mut paths: HashSet<String> = HashSet::new();
    let mut stats = ParseStats::default();

    for line in reader.lines() {
        let line = line.with_context(|| format!("cannot read log file {}", path.display()))?;
        stats.lines_total += 1;
        if let Some(p) = path_from_line(&line) {
            stats.lines_with_path += 1;
            paths.insert(p);
        }
    }

    Ok((paths, stats))
}

/// Извлекает HTTP-path из одной строки лога.
///
/// Возвращает `None`, если request-блок отсутствует или path извлечь нельзя.
pub fn path_from_line(line: &str) -> Option<String> {
    // первая пара двойных кавычек — request: "METHOD PATH HTTP/x.y"
    let start = line.find('"')?;
    let rest = &line[start + 1..];
    let end = rest.find('"')?;
    let request = &rest[..end];

    let mut parts = request.split_whitespace();
    let _method = parts.next()?;
    let raw_path = parts.next()?;

    normalize_target(raw_path)
}

/// Нормализует request-target:
/// * `*` (форма `OPTIONS *`) — не путь, отбрасывается;
/// * абсолютный URI `http://host/path` → `/path`;
/// * всё остальное возвращается как есть (path + query).
fn normalize_target(raw: &str) -> Option<String> {
    if raw == "*" {
        return None;
    }
    if let Some(idx) = raw.find("://") {
        let after_host = &raw[idx + 3..];
        return match after_host.find('/') {
            Some(slash) => Some(after_host[slash..].to_string()),
            None => Some("/".to_string()),
        };
    }
    Some(raw.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_path_from_combined_log() {
        let line = r#"94.154.43.229 - - [02/Aug/2026:02:09:30 +0000] "GET /index.php?option=com_jce&task=cpanel.feed HTTP/1.1" 200 1545 "-" "Mozilla/5.0""#;
        assert_eq!(
            path_from_line(line).as_deref(),
            Some("/index.php?option=com_jce&task=cpanel.feed")
        );
    }

    #[test]
    fn extracts_root_and_post() {
        let line = r#"1.2.3.4 - - [02/Aug/2026:02:09:30 +0000] "POST /api/login HTTP/1.1" 302 0 "-" "-""#;
        assert_eq!(path_from_line(line).as_deref(), Some("/api/login"));
    }

    #[test]
    fn skips_lines_without_request() {
        assert!(path_from_line("no quotes here").is_none());
        assert!(path_from_line("").is_none());
        assert_eq!(path_from_line(r#"1.2.3.4 - - [d] "-" 200 0 "-"- "-""#), None);
    }

    #[test]
    fn normalizes_absolute_uri() {
        assert_eq!(
            path_from_line(r#"h - - [d] "GET http://example.com/a/b HTTP/1.1" 200 1 "-""#)
                .as_deref(),
            Some("/a/b")
        );
        assert_eq!(
            path_from_line(r#"h - - [d] "GET http://example.com HTTP/1.1" 200 1 "-""#)
                .as_deref(),
            Some("/")
        );
    }

    #[test]
    fn drops_asterisk_request_target() {
        assert_eq!(
            path_from_line(r#"h - - [d] "OPTIONS * HTTP/1.1" 200 1 "-""#),
            None
        );
    }
}

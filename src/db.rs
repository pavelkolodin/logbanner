//! Файл «базы данных»: известные выражения, пути и статус.
//!
//! Формат строки файла — три колонки, разделённые пробелом:
//!
//! ```text
//! <expression> "<path>" <status>
//! NULL "<path>" BAD
//! == "<path>" BAD
//! ```
//!
//! * первая колонка — `NULL`, `<expression>` или зарезервированный маркер `==`;
//! * `<expression>` отделён пробелом от `<path>`;
//! * `<path>` — HTTP-путь в двойных кавычках;
//! * `NULL` — «мы пока не знаем, каким выражением ловить этот путь»;
//! * `==` — expression копируется из `<path>`: строка означает «путь
//!   срабатывает на самого себя»;
//! * `<status>` — любое строковое значение (обычно `BAD` или `GOOD`);
//!   если встречается другое значение — оно сохраняется как есть;
//!   если 3-я колонка в строке отсутствует — при чтении она приравнивается к `BAD`.

use crate::matcher;
use anyhow::{Context, Result};
use std::collections::HashSet;
use std::fs;
use std::path::Path;

/// Строковое обозначение неизвестного выражения.
pub const NULL_EXPRESSION: &str = "NULL";

/// Зарезервированный маркер первой колонки: expression копируется из `<path>`.
pub const SELF_EXPRESSION: &str = "==";

/// Значение 3-й колонки по умолчанию (когда колонки в строке нет).
pub const DEFAULT_STATUS: &str = "BAD";

/// Одна строка базы данных.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbRow {
    pub expression: String,
    pub path: String,
    /// 3-я колонка: любое строковое значение (обычно `BAD`/`GOOD`).
    pub status: String,
    /// Строка прочитана через зарезервированный маркер `==` в первой колонке:
    /// при перезаписи файла маркер возвращается на место.
    pub from_marker: bool,
}

impl DbRow {
    /// Новая запись со статусом по умолчанию (`BAD`).
    pub fn new(expression: impl Into<String>, path: impl Into<String>) -> Self {
        Self::with_status(expression, path, DEFAULT_STATUS)
    }

    /// Новая запись с явно заданным статусом.
    pub fn with_status(
        expression: impl Into<String>,
        path: impl Into<String>,
        status: impl Into<String>,
    ) -> Self {
        let status = status.into();
        let status = if status.is_empty() {
            DEFAULT_STATUS.to_string()
        } else {
            status
        };
        Self {
            expression: expression.into(),
            path: path.into(),
            status,
            from_marker: false,
        }
    }

    /// Новая запись «путь пока не покрыт ни одним выражением».
    pub fn unknown(path: impl Into<String>) -> Self {
        Self::new(NULL_EXPRESSION, path)
    }

    /// `true`, если выражение неизвестно (`NULL`).
    pub fn is_unknown(&self) -> bool {
        self.expression.eq_ignore_ascii_case(NULL_EXPRESSION)
    }

    /// Сериализация в строку файла: `<expression> "<path>" <status>`.
    ///
    /// Если строка прочитана через маркер `==`, первая колонка сохраняется
    /// как `==` — маркер не превращается в отдельную сущность при перезаписи.
    pub fn to_line(&self) -> String {
        let expression = if self.from_marker {
            SELF_EXPRESSION
        } else {
            self.expression.as_str()
        };
        format!("{} \"{}\" {}", expression, self.path, self.status)
    }

    /// Подходит ли `path` под эту строку базы.
    ///
    /// Если `<expression> == <path>` (строка прочитана через маркер `==`
    /// или записана буквально), движок `<expression>` **отключён намертво**:
    /// срабатывает только точное строковое равенство. Это защита от
    /// случайного `*` в самом пути (например, запрос вида
    /// `/\*/\*/\*\*\*\*sobaka\*/\*`): скопированный в expression path
    /// никогда не работает как широкий шаблон.
    pub fn matches_path(&self, path: &str) -> bool {
        if self.expression == self.path {
            self.path == path
        } else {
            matcher::matches(&self.expression, path)
        }
    }
}

/// Результат чтения файла базы.
#[derive(Debug)]
pub struct LoadOutcome {
    /// Записи после минимизации.
    pub rows: Vec<DbRow>,
    /// Сколько строк было схлопнуто при минимизации.
    pub collapsed: usize,
    /// Сколько строк не удалось разобрать (пропущены с предупреждением).
    pub skipped_lines: usize,
    /// Сколько строк получило подставленное значение статуса `BAD`
    /// (3-я колонка отсутствовала).
    pub status_defaults: usize,
    /// Существовал ли файл базы на диске.
    pub existed: bool,
}

/// Читает файл базы, парсит его и минимизирует (схлопывает) записи.
pub fn load(path: &Path) -> Result<LoadOutcome> {
    if !path.exists() {
        return Ok(LoadOutcome {
            rows: Vec::new(),
            collapsed: 0,
            skipped_lines: 0,
            status_defaults: 0,
            existed: false,
        });
    }

    let content = fs::read_to_string(path)
        .with_context(|| format!("cannot read db file {}", path.display()))?;

    let mut parsed: Vec<DbRow> = Vec::new();
    let mut skipped_lines = 0usize;
    let mut status_defaults = 0usize;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        match parse_line_parts(line) {
            Some((row, status_absent)) => {
                if status_absent {
                    status_defaults += 1;
                }
                parsed.push(row);
            }
            None => {
                skipped_lines += 1;
                eprintln!("logdigger: warning: skipping malformed db line: {trimmed}");
            }
        }
    }

    let (rows, collapsed) = collapse(parsed);

    Ok(LoadOutcome {
        rows,
        collapsed,
        skipped_lines,
        status_defaults,
        existed: true,
    })
}

/// Перезаписывает файл базы переданным списком записей.
///
/// Строки записываются в сгруппированном порядке (стабильная сортировка —
/// относительный порядок внутри группы сохраняется):
/// 1. выражения, в `<expression>` которых есть `*`;
/// 2. прочие строки с реальным `<expression>`;
/// 3. строки с маркером `==` в первой колонке (выключенные expressions);
/// 4. все строки с `NULL` в первой колонке — всегда последними.
pub fn save(path: &Path, rows: &[DbRow]) -> Result<()> {
    let mut ordered: Vec<&DbRow> = rows.iter().collect();
    ordered.sort_by_key(|row| sort_group(row));

    let mut buf = String::new();
    for row in ordered {
        buf.push_str(&row.to_line());
        buf.push('\n');
    }
    fs::write(path, buf)
        .with_context(|| format!("cannot write db file {}", path.display()))?;
    Ok(())
}

/// Группа строки для порядка в файле (меньше — выше в файле):
/// 0 — expression содержит `*` (в том числе экранированный `\*`);
/// 1 — прочие выражения; 2 — маркер `==`; 3 — `NULL`.
fn sort_group(row: &DbRow) -> u8 {
    if row.is_unknown() {
        3
    } else if row.from_marker {
        2
    } else if row.expression.contains('*') {
        0
    } else {
        1
    }
}

/// Парсит строку файла базы.
///
/// Возвращает запись `(expression, path, status)` и признак того, была ли
/// 3-я колонка отсутствующей (значение `BAD` подставлено при чтении).
///
/// `None` — пустая строка, комментарий (`#`) или не разобрать.
fn parse_line_parts(line: &str) -> Option<(DbRow, bool)> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    if let Some(parts) = parse_quoted(trimmed) {
        return Some(parts);
    }
    // Fallback: колонки, разделённые пробелом, без кавычек:
    // <expression> <path> [<status> ...]
    let tokens: Vec<&str> = trimmed.split_whitespace().collect();
    if tokens.len() < 2 {
        return None;
    }
    if tokens.len() == 2 {
        // 3-я колонка отсутствует → подставляем BAD
        Some((build_row(tokens[0], tokens[1], DEFAULT_STATUS), true))
    } else {
        // статус — всё, что после пути
        Some((build_row(tokens[0], tokens[1], &tokens[2..].join(" ")), false))
    }
}

/// Разбор вида `<expression> "<path>" [<status>]`:
/// путь — последняя пара двойных кавычек, выражение — всё перед ней,
/// статус — всё после неё.
fn parse_quoted(s: &str) -> Option<(DbRow, bool)> {
    let close = s.rfind('"')?;
    let open = s[..close].rfind('"')?;
    let path = &s[open + 1..close];
    if path.is_empty() {
        return None;
    }
    let mut expr = s[..open].trim();
    // выражение может быть само обёрнуто в кавычки: "/*.php" "/index.php"
    if expr.len() >= 2 && expr.starts_with('"') && expr.ends_with('"') {
        expr = &expr[1..expr.len() - 1];
    }
    let rest = s[close + 1..].trim();
    if rest.is_empty() {
        // 3-я колонка отсутствует → подставляем BAD
        Some((build_row(expr, path, DEFAULT_STATUS), true))
    } else {
        Some((build_row(expr, path, rest), false))
    }
}

fn build_row(expr: &str, path: &str, status: &str) -> DbRow {
    let (expression, from_marker) = if expr.is_empty()
        || expr.eq_ignore_ascii_case(NULL_EXPRESSION)
    {
        (NULL_EXPRESSION.to_string(), false)
    } else if expr == SELF_EXPRESSION {
        // Зарезервированный маркер: в памяти expression копируется из <path>,
        // «как если бы содержимое <path> стояло в первой колонке»;
        // сам маркер запоминается, чтобы пережить перезапись файла.
        (path.to_string(), true)
    } else {
        (expr.to_string(), false)
    };
    let mut row = DbRow::with_status(expression, path, status);
    row.from_marker = from_marker;
    row
}

/// Минимизация базы: каждое выражение существует только один раз,
/// устаревшие `NULL`-записи, уже покрытые известными выражениями, удаляются.
///
/// Две фазы (порядок строк в файле сохраняется):
///
/// * **Фаза A** — обход строк сверху вниз; строка с выражением отбирается,
///   если такое же выражение ещё не отобрано и её пример `<path>` не покрыт
///   ни одним уже отобранным выражением. Исключение: строка, у которой
///   `<expression> == <path>` (самосопоставление, в т.ч. из маркера `==`),
///   покрытием более широким выражением **не убирается** — она схлопывается
///   только при одинаковом `<path>` (дубликат выражения либо совпадение
///   пути с уже отобранной строкой).
/// * **Фаза B** — отобранные выражения остаются всегда (выражение не может
///   быть удалено собственной сработкой); каждая остальная `NULL`-строка
///   удаляется, если какое-либо отобранное выражение подходит к её `<path>`,
///   либо если такой путь уже встречался у другой `NULL`-строки.
///
/// 3-я колонка (статус) выживших строк сохраняется как есть.
///
/// Возвращает новые строки и количество удалённых.
pub fn collapse(rows: Vec<DbRow>) -> (Vec<DbRow>, usize) {
    let n = rows.len();

    // Фаза A: отбор строк с выражениями
    let mut selected = vec![false; n];
    let mut chosen: Vec<usize> = Vec::new();

    for (i, row) in rows.iter().enumerate() {
        if row.is_unknown() {
            continue;
        }
        let expr_dup = chosen.iter().any(|&j| rows[j].expression == row.expression);
        if expr_dup {
            continue;
        }
        let covered = chosen.iter().any(|&j| rows[j].matches_path(&row.path));
        if covered {
            if row.expression == row.path {
                // Строка «срабатывает на саму себя» (в т.ч. прочитанная через
                // маркер ==): схлопывается только если <path> одинаковый у уже
                // отобранной строки; более широкое выражение её не убирает.
                let same_path = chosen.iter().any(|&j| rows[j].path == row.path);
                if !same_path {
                    selected[i] = true;
                    chosen.push(i);
                }
            }
            continue;
        }
        selected[i] = true;
        chosen.push(i);
    }

    // Фаза B: сборка итогового списка
    let mut out: Vec<DbRow> = Vec::new();
    let mut seen_null: HashSet<&str> = HashSet::new();

    for (i, row) in rows.iter().enumerate() {
        if selected[i] {
            out.push(row.clone());
            continue;
        }
        if !row.is_unknown() {
            // дубликат выражения либо выражение, чей пример уже покрыт другим
            continue;
        }
        let covered = chosen.iter().any(|&j| rows[j].matches_path(&row.path));
        if covered {
            continue;
        }
        if !seen_null.insert(row.path.as_str()) {
            continue; // дубликат NULL-записи
        }
        out.push(row.clone());
    }

    let removed = n - out.len();
    (out, removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Обёртка для тестов: только запись, без признака подстановки статуса.
    fn parse_line(line: &str) -> Option<DbRow> {
        parse_line_parts(line).map(|(row, _)| row)
    }

    #[test]
    fn parses_standard_line() {
        let row = parse_line(r#"NULL "/index.php""#).unwrap();
        assert_eq!(row.expression, "NULL");
        assert_eq!(row.path, "/index.php");
        assert!(row.is_unknown());
        // 3-я колонка отсутствовала → подставлен BAD
        assert_eq!(row.status, "BAD");
    }

    #[test]
    fn parses_expression_line() {
        let row = parse_line(r#"/*.php "/index.php""#).unwrap();
        assert_eq!(row.expression, "/*.php");
        assert_eq!(row.path, "/index.php");
        assert!(!row.is_unknown());
        assert_eq!(row.status, "BAD");
    }

    #[test]
    fn parses_third_column_status() {
        assert_eq!(parse_line(r#"NULL "/x" GOOD"#).unwrap().status, "GOOD");
        assert_eq!(parse_line(r#"NULL "/x" BAD"#).unwrap().status, "BAD");
        // чужое значение сохраняется как есть
        assert_eq!(
            parse_line(r#"/*.php "/index.php" CUSTOM-1"#).unwrap().status,
            "CUSTOM-1"
        );
        // статус — всё, что стоит после пути
        assert_eq!(
            parse_line(r#"NULL "/x" MY VALUE"#).unwrap().status,
            "MY VALUE"
        );

        let (_, absent) = parse_line_parts(r#"NULL "/x""#).unwrap();
        assert!(absent, "нет 3-й колонки → считаем её отсутствующей");
        let (_, present) = parse_line_parts(r#"NULL "/x" GOOD"#).unwrap();
        assert!(!present);
    }

    #[test]
    fn parses_quoted_expression() {
        let row = parse_line(r#""/*.php" "/index.php""#).unwrap();
        assert_eq!(row.expression, "/*.php");
        assert_eq!(row.path, "/index.php");
    }

    #[test]
    fn parses_expression_with_asterisks_anywhere() {
        let row = parse_line(r#"*index*php* "/index.php""#).unwrap();
        assert_eq!(row.expression, "*index*php*");
        let row = parse_line(r#"/a*b "/ab""#).unwrap();
        assert_eq!(row.expression, "/a*b");
    }

    #[test]
    fn fallback_without_quotes() {
        let row = parse_line("NULL /robots.txt").unwrap();
        assert!(row.is_unknown());
        assert_eq!(row.path, "/robots.txt");
        assert_eq!(row.status, "BAD"); // колонка отсутствовала
        // и со статусом
        let row = parse_line("NULL /robots.txt GOOD").unwrap();
        assert_eq!(row.status, "GOOD");
    }

    #[test]
    fn skips_comments_empty_and_malformed() {
        assert!(parse_line("").is_none());
        assert!(parse_line("   ").is_none());
        assert!(parse_line("# comment").is_none());
        assert!(parse_line("onlyoneword").is_none());
    }

    #[test]
    fn parses_double_equals_marker() {
        // маркер == : expression копируется из <path> (в памяти)
        let (row, status_absent) = parse_line_parts(r#"== "/foo""#).unwrap();
        assert_eq!(row.expression, "/foo");
        assert_eq!(row.path, "/foo");
        assert!(!row.is_unknown());
        assert!(status_absent);
        assert_eq!(row.status, "BAD");

        // вместе со статусом
        let row = parse_line(r#"== "/a/b" GOOD"#).unwrap();
        assert_eq!(row.expression, "/a/b");
        assert_eq!(row.path, "/a/b");
        assert_eq!(row.status, "GOOD");

        // fallback без кавычек
        let row = parse_line("== /robots.txt").unwrap();
        assert_eq!(row.expression, "/robots.txt");
        assert_eq!(row.path, "/robots.txt");

        // при сохранении маркер == остаётся в первой колонке
        assert_eq!(row.to_line(), r#"== "/robots.txt" BAD"#);
        assert_eq!(parse_line(&row.to_line()).unwrap(), row);
        assert!(parse_line(&row.to_line()).unwrap().from_marker);

        // литеральная запись (без маркера) остаётся литеральной
        let row = parse_line(r#"/foo "/foo" GOOD"#).unwrap();
        assert!(!row.from_marker);
        assert_eq!(row.to_line(), r#"/foo "/foo" GOOD"#);
    }

    #[test]
    fn double_equals_disables_engine_for_the_row() {
        // path со случайной '*' после копирования: движок отключён намертво,
        // срабатывает только точное строковое равенство
        let row = parse_line(r#"== "/a*b""#).unwrap();
        assert_eq!(row.expression, "/a*b");
        assert!(row.matches_path("/a*b"));
        assert!(!row.matches_path("/aXb"));
        assert!(!row.matches_path("/ab"));

        // бредовый путь из задания — не даёт широких срабатываний
        let weird = r#"/\*/\*/\*\*\*\*sobaka\*/\*"#;
        let row = parse_line(&format!("== \"{}\"", weird)).unwrap();
        assert_eq!(row.expression, weird);
        assert!(row.matches_path(weird));
        assert!(!row.matches_path("/anything"));
        assert!(!row.matches_path(r"/x/x/xxxxsobakax/x"));

        // даже «широко выглядящий» ==-путь ловит только сам себя
        let row = parse_line(r#"== "/*""#).unwrap();
        assert!(row.matches_path("/*"));
        assert!(!row.matches_path("/index.php"));

        // литеральная запись path в первой колонке ведёт себя так же
        let row = parse_line(r#"/a*b "/a*b""#).unwrap();
        assert!(row.matches_path("/a*b"));
        assert!(!row.matches_path("/aXb"));
    }

    #[test]
    fn exact_row_covers_only_identical_paths() {
        // == "/a*b" не схлопывает NULL с другим путём, который "ловился бы"
        // по wildcard, — покрытие работает только при точном равенстве путей
        let rows = vec![
            DbRow::new("/a*b", "/a*b"),
            DbRow::unknown("/aXb"),
            DbRow::unknown("/a*b"),
        ];
        let (out, removed) = collapse(rows);
        assert_eq!(removed, 1); // только NULL "/a*b"
        assert_eq!(out.len(), 2);
        assert!(out.iter().any(|r| r.path == "/aXb" && r.is_unknown()));
        assert!(out.iter().any(|r| r.path == "/a*b" && !r.is_unknown()));
    }

    #[test]
    fn row_sort_groups() {
        // 0 — expression с '*'
        assert_eq!(sort_group(&parse_line(r#"*.php "/x""#).unwrap()), 0);
        assert_eq!(sort_group(&parse_line(r#"/*.env "/y""#).unwrap()), 0);
        assert_eq!(sort_group(&parse_line(r#"/a\*b "/lit*""#).unwrap()), 0); // и '\*'
        // 1 — прочие выражения
        assert_eq!(sort_group(&parse_line(r#"/foo "/bar""#).unwrap()), 1);
        assert_eq!(sort_group(&parse_line(r#"/foo "/foo""#).unwrap()), 1);
        // 2 — маркер == (важнее, чем '*' в скопированном пути)
        assert_eq!(sort_group(&parse_line(r#"== "/eq""#).unwrap()), 2);
        assert_eq!(sort_group(&parse_line(r#"== "/*""#).unwrap()), 2);
        assert_eq!(sort_group(&parse_line(r#"== "/a*b""#).unwrap()), 2);
        // 3 — NULL всегда последними
        assert_eq!(sort_group(&DbRow::unknown("/n")), 3);
        assert_eq!(sort_group(&parse_line(r#"NULL "/n" GOOD"#).unwrap()), 3);
    }

    #[test]
    fn row_serialization_roundtrip() {
        let row = DbRow::unknown("/a b".to_string());
        assert_eq!(row.to_line(), r#"NULL "/a b" BAD"#);
        let back = parse_line(&row.to_line()).unwrap();
        assert_eq!(back, row);

        let row = DbRow::with_status("/*.php", "/x", "GOOD");
        assert_eq!(row.to_line(), r#"/*.php "/x" GOOD"#);
        assert_eq!(parse_line(&row.to_line()).unwrap(), row);
    }

    #[test]
    fn parses_escaped_expression_and_roundtrip() {
        // \* — литеральный asterisk, \\ — литеральный backslash
        let row = parse_line(r#"\* "*""#).unwrap();
        assert_eq!(row.expression, "\\*");
        assert_eq!(row.path, "*");
        let row2 = parse_line(r#"\\ "/dir\file""#).unwrap();
        // поле expression хранится как есть (два символа '\'), без раскрытия escape
        assert_eq!(row2.expression, "\\\\");
        assert_eq!(row2.path, "/dir\\file");
        // обратная сериализация сохраняет escape-последовательности как есть
        assert_eq!(row.to_line(), r#"\* "*" BAD"#);
        assert_eq!(parse_line(&row.to_line()).unwrap(), row);
    }

    #[test]
    fn collapse_removes_null_covered_by_expression() {
        let rows = vec![
            DbRow::unknown("/index.php"),
            DbRow::new("/*.php", "/old.php"),
            DbRow::unknown("/robots.txt"),
        ];
        let (out, removed) = collapse(rows);
        assert_eq!(removed, 1);
        // NULL "/index.php" покрыт /*.php; NULL "/robots.txt" не покрыт
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].expression, "/*.php");
        assert_eq!(out[1].path, "/robots.txt");
    }

    #[test]
    fn collapse_handles_null_before_covering_expression() {
        // NULL-строка стоит ДО выражения, которое её покрывает —
        // двухфазный алгоритм обязан её убрать.
        let rows = vec![
            DbRow::unknown("/a.php"),
            DbRow::new("/*.php", "/b.php"),
        ];
        let (out, removed) = collapse(rows);
        assert_eq!(removed, 1);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].expression, "/*.php");
    }

    #[test]
    fn collapse_deduplicates_expressions() {
        let rows = vec![
            DbRow::new("/*.env", "/.env"),
            DbRow::new("/*.env", "/api/.env"),
            DbRow::new("/*.env", "/admin/.env"),
        ];
        let (out, removed) = collapse(rows);
        assert_eq!(removed, 2);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].expression, "/*.env");
        assert_eq!(out[0].path, "/.env");
    }

    #[test]
    fn collapse_preserves_status_of_kept_rows() {
        let rows = vec![
            DbRow::with_status("/*.env", "/.env", "GOOD"),
            DbRow::with_status("NULL", "/keep", "WEIRD"),
        ];
        let (out, removed) = collapse(rows);
        assert_eq!(removed, 0);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].status, "GOOD");
        assert_eq!(out[1].status, "WEIRD");
    }

    #[test]
    fn collapse_never_deletes_expression_by_its_own_match() {
        let rows = vec![DbRow::new("/*.php", "/index.php")];
        let (out, removed) = collapse(rows);
        assert_eq!(removed, 0);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn collapse_deduplicates_null_paths() {
        let rows = vec![DbRow::unknown("/x"), DbRow::unknown("/x"), DbRow::unknown("/y")];
        let (out, removed) = collapse(rows);
        assert_eq!(removed, 1);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn collapse_is_idempotent() {
        let rows = vec![
            DbRow::unknown("/index.php"),
            DbRow::new("/*.php", "/old.php"),
            DbRow::unknown("/robots.txt"),
        ];
        let (once, removed) = collapse(rows);
        assert!(removed > 0);
        let (twice, removed2) = collapse(once.clone());
        assert_eq!(removed2, 0);
        assert_eq!(once, twice);
    }

    #[test]
    fn collapse_respects_escaped_asterisk() {
        // Выражение \* ловит только литеральный '*' в пути.
        let rows = vec![
            DbRow::new("\\*", "*"),
            DbRow::unknown("*"),
            DbRow::unknown("/normal"),
        ];
        let (out, removed) = collapse(rows);
        assert_eq!(removed, 1); // только NULL "*" покрыт выражением \*
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].expression, "\\*");
        assert_eq!(out[1].path, "/normal");
    }

    #[test]
    fn self_match_row_not_removed_by_broader_expression() {
        // строки «на самого себя» (expression == path, в т.ч. из "==")
        // не схлопываются более широким выражением с ДРУГИМ путём примера
        let rows = vec![
            DbRow::new("*", "/anything"),
            DbRow::new("/foo", "/foo"), // то, что прочитано из == "/foo"
        ];
        let (out, removed) = collapse(rows);
        assert_eq!(removed, 0);
        assert_eq!(out.len(), 2);
        assert_eq!(out[1].expression, "/foo");
        assert_eq!(out[1].path, "/foo");
    }

    #[test]
    fn self_match_row_collapses_on_identical_path_only() {
        // путь у покрывающей отобранной строки совпадает → схлопывается
        let rows = vec![DbRow::new("/*", "/foo"), DbRow::new("/foo", "/foo")];
        let (out, removed) = collapse(rows);
        assert_eq!(removed, 1);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].expression, "/*");

        // два одинаковых self-строки → дубликат выражения (пути идентичны)
        let rows = vec![DbRow::new("/foo", "/foo"), DbRow::new("/foo", "/foo")];
        let (out, removed) = collapse(rows);
        assert_eq!(removed, 1);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].path, "/foo");
    }

    #[test]
    fn self_match_row_still_absorbs_null_with_same_path() {
        let rows = vec![
            DbRow::new("*", "/anything"),
            DbRow::new("/foo", "/foo"),
            DbRow::unknown("/foo"),
        ];
        let (out, removed) = collapse(rows);
        // NULL "/foo" покрыт — убран; self-строка осталась
        assert_eq!(removed, 1);
        assert_eq!(out.len(), 2);
        assert!(out.iter().any(|r| r.expression == "/foo" && r.path == "/foo"));
    }
}

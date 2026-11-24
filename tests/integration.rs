//! Интеграционный тест: полный цикл работы бинарника logdigger.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "logdigger-it-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run(args: &[&Path]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_logdigger"))
        .args(args.iter().map(|p| p.as_os_str()))
        .output()
        .expect("failed to run logdigger")
}

#[test]
fn full_cycle_collapse_add_and_idempotency() {
    let tmp = TempDir::new();
    let log = tmp.path().join("in.log");
    let db = tmp.path().join("db.txt");

    fs::write(
        &log,
        concat!(
            r#"1.2.3.4 - - [02/Aug/2026:02:09:30 +0000] "GET /index.php HTTP/1.1" 200 10 "-" "x""#,
            "\n",
            r#"1.2.3.4 - - [02/Aug/2026:02:09:31 +0000] "GET /new/path HTTP/1.1" 200 10 "-" "x""#,
            "\n",
            r#"1.2.3.4 - - [02/Aug/2026:02:09:32 +0000] "GET /index.php HTTP/1.1" 200 10 "-" "x""#,
            "\n",
        ),
    )
    .unwrap();

    // NULL "/index.php" уже покрыт выражением /*.php → должен схлопнуться
    fs::write(&db, "NULL \"/index.php\"\n/*.php \"/old.php\"\n").unwrap();

    let out = run(&[Path::new("--in"), &log, Path::new("--db"), &db]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("1 collapsed"), "stdout: {stdout}");

    let content = fs::read_to_string(&db).unwrap();
    // NULL "/index.php" схлопнут, выражение сохранилось, добавлен новый NULL
    assert!(!content.contains("NULL \"/index.php\""), "content: {content}");
    assert!(content.contains("/*.php \"/old.php\""), "content: {content}");
    assert!(content.contains("NULL \"/new/path\""), "content: {content}");
    // /index.php совпал с выражением /*.php → новая NULL-запись не добавлена
    assert!(!content.contains("/index.php"), "content: {content}");

    // Повторный запуск: изменений нет, файл не перезаписывается
    let first = content;
    let out2 = run(&[Path::new("--in"), &log, Path::new("--db"), &db]);
    assert!(out2.status.success());
    let stdout2 = String::from_utf8_lossy(&out2.stdout);
    assert!(stdout2.contains("no changes"), "stdout: {stdout2}");
    assert_eq!(fs::read_to_string(&db).unwrap(), first);
}

#[test]
fn runs_without_in_argument() {
    let tmp = TempDir::new();
    let db = tmp.path().join("db.txt");
    fs::write(&db, "/*.php \"/a.php\"\n/*.php \"/b.php\"\nNULL \"/a.php\"\n").unwrap();

    let out = run(&[Path::new("--db"), &db]);
    assert!(out.status.success());
    let content = fs::read_to_string(&db).unwrap();
    // дубликат выражения и покрытый NULL убраны, отсутствующий статус → BAD
    assert_eq!(content, "/*.php \"/a.php\" BAD\n");
}

#[test]
fn missing_status_column_defaulted_to_bad_and_others_preserved() {
    let tmp = TempDir::new();
    let db = tmp.path().join("db.txt");
    fs::write(
        &db,
        "/*.php \"/a.php\"\nNULL \"/b\" GOOD\nNULL \"/c\" WEIRD-VALUE\n",
    )
    .unwrap();

    // 1-й запуск: отсутствующая 3-я колонка становится BAD, остальные — как есть
    let out = run(&[Path::new("--db"), &db]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("1 line(s) got default status BAD"));
    let content = fs::read_to_string(&db).unwrap();
    assert_eq!(
        content,
        "/*.php \"/a.php\" BAD\nNULL \"/b\" GOOD\nNULL \"/c\" WEIRD-VALUE\n"
    );

    // 2-й запуск: изменений больше нет
    let out2 = run(&[Path::new("--db"), &db]);
    assert!(String::from_utf8_lossy(&out2.stdout).contains("no changes"));
    assert_eq!(fs::read_to_string(&db).unwrap(), content);
}

#[test]
fn double_equals_marker_rows_survive_broader_expressions() {
    let tmp = TempDir::new();
    let db = tmp.path().join("db.txt");
    fs::write(&db, "* \"/anything\"\n== \"/foo\"\n== \"/bar\"\n").unwrap();

    let out = run(&[Path::new("--db"), &db]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let content = fs::read_to_string(&db).unwrap();
    // широкое выражение НЕ схлопывает self-строки с другими путями;
    // маркер == при сохранении остаётся в первой колонке
    assert_eq!(
        content,
        "* \"/anything\" BAD\n== \"/foo\" BAD\n== \"/bar\" BAD\n"
    );

    // идемпотентность повторного запуска
    let out2 = run(&[Path::new("--db"), &db]);
    assert!(String::from_utf8_lossy(&out2.stdout).contains("no changes"));
    assert_eq!(fs::read_to_string(&db).unwrap(), content);
}

#[test]
fn double_equals_marker_collapses_on_identical_path() {
    let tmp = TempDir::new();
    let db = tmp.path().join("db.txt");
    fs::write(&db, "/* \"/foo\"\n== \"/foo\" GOOD\nNULL \"/foo\" BAD\n").unwrap();

    let out = run(&[Path::new("--db"), &db]);
    assert!(out.status.success());
    let content = fs::read_to_string(&db).unwrap();
    // self-строка и NULL с тем же путём схлопнулись в покрывающую строку
    assert_eq!(content, "/* \"/foo\" BAD\n");
}

#[test]
fn double_equals_row_matches_only_exactly_in_log() {
    let tmp = TempDir::new();
    let log = tmp.path().join("in.log");
    let db = tmp.path().join("db.txt");
    fs::write(
        &log,
        concat!(
            r#"1.2.3.4 - - [02/Aug/2026:02:09:30 +0000] "GET /se*arch HTTP/1.1" 200 10 "-" "x""#,
            "\n",
            r#"1.2.3.4 - - [02/Aug/2026:02:09:31 +0000] "GET /seXarch HTTP/1.1" 200 10 "-" "x""#,
            "\n",
        ),
    )
    .unwrap();
    fs::write(&db, "== \"/se*arch\" BAD\n").unwrap();

    let out = run(&[Path::new("--in"), &log, Path::new("--db"), &db]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let content = fs::read_to_string(&db).unwrap();
    // движок отключён: '*' в ==-пути не работает как wildcard —
    // /seXarch не совпал и ушёл в NULL, а точный /se*arch совпал;
    // маркер == при сохранении остаётся в первой колонке
    assert_eq!(
        content,
        "== \"/se*arch\" BAD\nNULL \"/seXarch\" BAD\n"
    );

    // идемпотентность
    let out2 = run(&[Path::new("--in"), &log, Path::new("--db"), &db]);
    assert!(String::from_utf8_lossy(&out2.stdout).contains("no changes"));
    assert_eq!(fs::read_to_string(&db).unwrap(), content);
}

#[test]
fn db_rewrite_groups_rows_by_first_column() {
    let tmp = TempDir::new();
    let db = tmp.path().join("db.txt");
    // подставленный BAD у последней строки вызывает перезапись
    fs::write(
        &db,
        "NULL \"/z\" BAD\n== \"/eq\" BAD\n/foo \"/foo\" BAD\n*.php \"/x.php\" BAD\n/*\\* \"/lit*\" BAD\nNULL \"/a\"\n",
    )
    .unwrap();

    let out = run(&[Path::new("--db"), &db]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let content = fs::read_to_string(&db).unwrap();
    // порядок групп: выражения с '*' → прочие выражения → == → все NULL;
    // внутри группы порядок исходный (стабильность)
    assert_eq!(
        content,
        "*.php \"/x.php\" BAD\n/*\\* \"/lit*\" BAD\n/foo \"/foo\" BAD\n== \"/eq\" BAD\nNULL \"/z\" BAD\nNULL \"/a\" BAD\n"
    );

    // идемпотентность: повторный запуск ничего не меняет
    let out2 = run(&[Path::new("--db"), &db]);
    assert!(String::from_utf8_lossy(&out2.stdout).contains("no changes"));
    assert_eq!(fs::read_to_string(&db).unwrap(), content);
}

#[test]
fn new_null_rows_from_log_get_bad_status() {
    let tmp = TempDir::new();
    let log = tmp.path().join("in.log");
    let db = tmp.path().join("db.txt");
    fs::write(
        &log,
        r#"1.2.3.4 - - [02/Aug/2026:02:09:30 +0000] "GET /brand/new HTTP/1.1" 200 10 "-" "x""#,
    )
    .unwrap();
    fs::write(&db, "/*.php \"/old.php\" GOOD\n").unwrap();

    let out = run(&[Path::new("--in"), &log, Path::new("--db"), &db]);
    assert!(out.status.success());
    let content = fs::read_to_string(&db).unwrap();
    // новая NULL-запись получает BAD; существующий GOOD не тронут
    assert_eq!(content, "/*.php \"/old.php\" GOOD\nNULL \"/brand/new\" BAD\n");
}

#[test]
fn missing_db_argument_fails() {
    let out = Command::new(env!("CARGO_BIN_EXE_logdigger"))
        .output()
        .unwrap();
    assert!(!out.status.success());
}

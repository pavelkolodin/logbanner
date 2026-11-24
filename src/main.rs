mod cli;
mod db;
mod logparse;
mod matcher;

use anyhow::Result;
use clap::Parser;
use std::collections::HashSet;

fn main() -> Result<()> {
    let cli = cli::Cli::parse();

    // Шаг 1: чтение и парсинг файла --db.
    let loaded = db::load(&cli.db)?;
    println!(
        "db: {} row(s) loaded, {} collapsed, {} malformed line(s) skipped{}{}",
        loaded.rows.len(),
        loaded.collapsed,
        loaded.skipped_lines,
        if loaded.existed {
            ""
        } else {
            " (file not found, starting with empty db)"
        },
        if loaded.status_defaults > 0 {
            format!(
                ", {} line(s) got default status {}",
                loaded.status_defaults,
                db::DEFAULT_STATUS
            )
        } else {
            String::new()
        }
    );

    let mut rows = loaded.rows;
    // изменения базы: схлопывание записей, подстановка статуса по умолчанию,
    // новые NULL-записи из лога
    let mut changed = loaded.collapsed > 0 || loaded.status_defaults > 0;

    // Шаг 5: разбор входного лога и проверка уникальных path по базе.
    if let Some(ref input) = cli.input {
        let (paths, stats) = logparse::extract_unique_paths(input)?;
        println!(
            "log: {} line(s), {} with request, {} unique path(s)",
            stats.lines_total,
            stats.lines_with_path,
            paths.len()
        );

        // какие-то пути уже есть в базе (примеры выражений / NULL-записи)
        let mut known: HashSet<String> = rows.iter().map(|r| r.path.clone()).collect();

        // детерминированный порядок добавления
        let mut unique: Vec<&String> = paths.iter().collect();
        unique.sort();

        let mut matched = 0usize;
        let mut already_in_db = 0usize;
        let mut added = 0usize;

        for path in unique {
            // каждый уникальный path прогоняется через все известные выражения;
            // строки с expression == path работают без движка (точное равенство)
            let hit = rows
                .iter()
                .any(|row| !row.is_unknown() && row.matches_path(path));
            if hit {
                matched += 1;
                continue;
            }
            if known.contains(path) {
                already_in_db += 1;
                continue;
            }
            // выражение придумывать не будем: только NULL-запись
            rows.push(db::DbRow::unknown(path.clone()));
            known.insert(path.clone());
            added += 1;
        }

        changed |= added > 0;
        println!(
            "log: {matched} path(s) matched by expressions, {already_in_db} already in db, {added} new NULL row(s)"
        );
    }

    // Шаг 6: перезапись файла --db только при изменениях.
    if changed {
        db::save(&cli.db, &rows)?;
        println!("db: rewritten, {} row(s) total", rows.len());
    } else {
        println!("db: no changes, file untouched, {} row(s)", rows.len());
    }

    Ok(())
}

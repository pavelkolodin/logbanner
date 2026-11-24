use clap::Parser;
use std::path::PathBuf;

/// logdigger — разбор сырых логов nginx и поддержка «базы» известных HTTP-путей
/// и выражений со звёздочкой.
#[derive(Debug, Parser)]
#[command(name = "logdigger", version, about, long_about = None)]
pub struct Cli {
    /// Входной файл с сырыми логами nginx
    #[arg(long = "in", value_name = "FILENAME")]
    pub input: Option<PathBuf>,

    /// Файл «базы данных»: строки вида <expression> "<path>" или NULL "<path>"
    #[arg(long = "db", value_name = "FILENAME")]
    pub db: PathBuf,
}

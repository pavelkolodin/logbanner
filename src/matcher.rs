//! Движок сопоставления `<expression>` с `<path>`.
//!
//! Поддерживается:
//! * символ `*` (wildcard) — «любое число любых символов, минимум ноль»;
//! * экранирование через backslash: `\X` в выражении означает **одиночный
//!   литеральный символ `X`** для сравнения. В частности `\*` сравнивает
//!   литеральный `*`, а `\\` — литеральный `\`. Одиночный `\` в конце
//!   выражения — литеральный `\`.
//!
//! Настоящий движок regular expressions намеренно не используется.
//!
//! Выражение сначала компилируется в последовательность токенов
//! (литерал / wildcard), затем идёт классический жадный обход двумя
//! указателями с откатом к последней встреченной wildcard (см. шаг 4 README).

/// Токен скомпилированного выражения.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    /// Одиночный символ: должен точно совпасть со символом path.
    Literal(char),
    /// Неэкранированный `*`: любое число любых символов, минимум ноль.
    Any,
}

/// Превращает строку выражения в последовательность токенов.
///
/// Правила разбора:
/// * `\X` → литерал `X` (любой символ после backslash — одиночный символ);
/// * одиночный `\` в конце выражения → литерал `\`;
/// * неэкранированный `*` → wildcard;
/// * любой другой символ → литерал самого себя.
fn compile(pattern: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some(escaped) => tokens.push(Token::Literal(escaped)),
                None => tokens.push(Token::Literal('\\')),
            },
            '*' => tokens.push(Token::Any),
            other => tokens.push(Token::Literal(other)),
        }
    }
    tokens
}

/// Возвращает `true`, если `text` подходит под шаблон `pattern`.
///
/// * `pattern` — выражение из файла базы (`<expression>`);
/// * `text` — HTTP-путь (`<path>`).
pub fn matches(pattern: &str, text: &str) -> bool {
    let tokens = compile(pattern);
    let t: Vec<char> = text.chars().collect();

    let mut pi = 0usize; // указатель по токенам pattern
    let mut ti = 0usize; // указатель по text
    let mut star: Option<usize> = None; // позиция последней встреченной wildcard
    let mut mark = 0usize; // до какого символа text съела последняя wildcard

    while ti < t.len() {
        if pi < tokens.len() {
            match tokens[pi] {
                Token::Literal(c) if c == t[ti] => {
                    // точное совпадение литерала — идём вперёд
                    pi += 1;
                    ti += 1;
                    continue;
                }
                Token::Any => {
                    // wildcard: запоминаем откат и пробуем съесть 0 символов
                    star = Some(pi);
                    mark = ti;
                    pi += 1;
                    continue;
                }
                _ => {}
            }
        }
        // несовпадение литерала (или конец pattern) — откат к последней '*'
        if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }

    // хвост pattern может состоять только из wildcard (они дают пустую строку)
    while pi < tokens.len() {
        match tokens[pi] {
            Token::Any => pi += 1,
            _ => break,
        }
    }
    pi == tokens.len()
}

#[cfg(test)]
mod tests {
    use super::matches;

    #[test]
    fn exact_match() {
        assert!(matches("/index.php", "/index.php"));
        assert!(matches("/", "/"));
    }

    #[test]
    fn no_match_without_wildcard() {
        // из примера задания: срабатывания нет
        assert!(!matches("/index.php", "/index.php.test"));
        assert!(!matches("/index.php", "/xindex.php"));
        assert!(!matches("", "/x"));
    }

    #[test]
    fn trailing_asterisk() {
        assert!(matches("/index.php*", "/index.php"));
        assert!(matches("/index.php*", "/index.php.test"));
        // '*' в конце = любой суффикс, включая пустой
        assert!(matches("/index.php*", "/index.phpx/other"));
        assert!(!matches("/index.php*", "/xindex.php"));
    }

    #[test]
    fn leading_asterisk() {
        assert!(matches("*.php", "/index.php"));
        assert!(matches("*", "/anything/at/all"));
        assert!(!matches("*.php", "/index.php?x=1"));
        assert!(matches("*", ""));
    }

    #[test]
    fn middle_asterisk() {
        assert!(matches("/a*b", "/ab"));
        assert!(matches("/a*b", "/aXXXb"));
        assert!(!matches("/a*b", "/aXXXc"));
        assert!(matches("/a*b*c", "/abc"));
        assert!(!matches("/a*b*c", "/ac")); // нужен литеральный 'b'
    }

    #[test]
    fn multiple_asterisks_and_backtracking() {
        // '*' в середине должна уступать место последующим литералам
        assert!(matches("*.env", "/.env"));
        assert!(matches("/*.env", "/api/.env"));
        assert!(matches("*a*b*", "aab"));
        assert!(matches("*a*b*", "xxaxxbxx"));
        assert!(!matches("*a*b*", "xxaxxcxx"));
        // звёздочка в pattern никогда не сравнивается как литерал
        assert!(matches("*", "*literal*"));
    }

    #[test]
    fn empty_pattern_and_text() {
        assert!(matches("", ""));
        assert!(!matches("", "/path"));
        assert!(matches("*", ""));
    }

    #[test]
    fn unicode_paths() {
        assert!(matches("/путь*", "/путь/файл"));
        assert!(matches("*/док", "/раздел/док"));
    }

    // ---- экранирование через backslash ----

    #[test]
    fn escaped_asterisk_is_literal() {
        // пример из задания: \* = *
        assert!(matches("\\*", "*"));
        assert!(!matches("\\*", "/index"));
        assert!(matches("/a\\*b", "/a*b"));
        assert!(!matches("/a\\*b", "/axb")); // литеральный '*', а не wildcard
        assert!(matches("*\\*", "abc*"));
        assert!(matches("*\\*", "ab*c*"));
        assert!(!matches("*\\*", "ab*c")); // не заканчивается на литеральный '*'
        assert!(!matches("*\\*", "abc"));
        // одиночный литеральный '*' в path не совпадает с wildcard-шаблоном без неё
        assert!(!matches("/x", "*"));
        assert!(matches("*", "*"));
    }

    #[test]
    fn escaped_backslash_is_literal() {
        // пример из задания: \\ = \
        assert!(matches("\\\\", "\\"));
        assert!(!matches("\\\\", "x"));
        assert!(!matches("\\\\", "*")); // одиночный '\' ≠ '*'
        // литеральный '\' + wildcard
        assert!(matches("\\\\*", "\\anything"));
        assert!(matches("\\\\*", "\\"));
        assert!(!matches("\\\\*", "/anything"));
        assert!(matches("*\\\\*", "a\\b"));
        // выражение /dir\\file (двойной backslash) ловит путь /dir\file
        assert!(matches("/dir\\\\file", "/dir\\file"));
        // '\f' в выражении = литеральный 'f', сам backslash съедается экранированием
        assert!(matches("/dir\\file", "/dirfile"));
        assert!(!matches("/dir\\file", "/dir\\file"));
    }

    #[test]
    fn escape_makes_next_char_literal_even_if_not_special() {
        assert!(matches("\\a", "a"));
        assert!(!matches("\\a", "\\a")); // '\' съеден экранированием
        assert!(matches("\\/", "/"));
    }

    #[test]
    fn trailing_lone_backslash_is_literal() {
        assert!(matches("\\", "\\"));
        assert!(!matches("\\", "p"));
        assert!(matches("/path\\", "/path\\"));
        assert!(!matches("/path\\", "/path/"));
    }

    #[test]
    fn escaped_asterisk_then_wildcard() {
        // \\* в выражении = литеральный '\' + wildcard
        assert!(matches("\\\\*", "\\upload"));
        // \** в выражении = литеральный '*' + wildcard
        assert!(matches("\\**", "*x*"));
        assert!(matches("\\**", "*"));
        assert!(!matches("\\**", "x*")); // путь должен начинаться с литерального '*'
    }
}

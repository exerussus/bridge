//! Игнор: файлы и папки, которых для моста «нет» — не показываются, не передаются, не принимаются.
//!
//! Шаблоны (регистр не важен):
//! - `имя`, `*.bak` — без «/»: файл или папка с таким именем на любой глубине (и всё внутри такой папки);
//! - `Сборки/` — «/» в конце: только папка с таким именем на любой глубине;
//! - `док/черновики`, `/корень.txt`, `отчёты/**/*.log` — с «/» внутри: путь от корня (и всё внутри, если это папка);
//! - `#` в начале — комментарий.

use crate::path::glob;

#[derive(Clone, Debug, Default)]
pub struct IgnoreRules {
    rules: Vec<Rule>,
}

#[derive(Clone, Debug)]
struct Rule {
    source: String,
    body: String,
    dir_only: bool,
    anchored: bool,
}

impl IgnoreRules {
    pub fn new<I, S>(patterns: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut rules: Vec<Rule> = Vec::new();
        for raw in patterns {
            let Some(p) = clean(raw.as_ref()) else { continue };
            if rules.iter().any(|r| r.source.eq_ignore_ascii_case(&p) || r.source.to_lowercase() == p.to_lowercase()) {
                continue;
            }
            let dir_only = p.ends_with('/');
            let body = p.trim_matches('/').to_string();
            let anchored = p.starts_with('/') || body.contains('/');
            rules.push(Rule { source: p, body, dir_only, anchored });
        }
        IgnoreRules { rules }
    }

    pub fn empty() -> Self {
        IgnoreRules { rules: Vec::new() }
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    pub fn patterns(&self) -> Vec<String> {
        self.rules.iter().map(|r| r.source.clone()).collect()
    }

    pub fn is_ignored(&self, path: &str) -> bool {
        self.which(path).is_some()
    }

    /// Какой шаблон сработал.
    pub fn which(&self, path: &str) -> Option<&str> {
        if self.rules.is_empty() || path.is_empty() {
            return None;
        }
        for r in &self.rules {
            let hit = if !r.anchored {
                let parts: Vec<&str> = path.split('/').collect();
                let last = if r.dir_only { parts.len() - 1 } else { parts.len() };
                parts[..last].iter().any(|seg| glob(&r.body, seg))
            } else {
                (!r.dir_only && glob(&r.body, path)) || glob(&format!("{}/**", r.body), path)
            };
            if hit {
                return Some(&r.source);
            }
        }
        None
    }
}

/// Нормальная форма шаблона (None — пусто или комментарий).
pub fn clean(raw: &str) -> Option<String> {
    let mut p = raw.trim().replace('\\', "/");
    if p.is_empty() || p.starts_with('#') {
        return None;
    }
    while p.contains("//") {
        p = p.replace("//", "/");
    }
    Some(p)
}

/// Ошибки в наборе шаблонов.
pub fn validate(patterns: &[String], where_: &str) -> Vec<String> {
    let mut e = Vec::new();
    let mut n = 0;
    for raw in patterns {
        let Some(p) = clean(raw) else { continue };
        n += 1;
        if p.chars().count() > 300 {
            e.push(format!("{}: шаблон длиннее 300 знаков", where_));
        }
        if p.split('/').any(|s| s == "..") {
            e.push(format!("{}: «..» в шаблоне «{}»", where_, p));
        }
        if p.trim_matches(|c| c == '/' || c == '*').is_empty() {
            e.push(format!("{}: шаблон «{}» скрыл бы всё", where_, p));
        }
        if p.contains([':', '<', '>', '|', '"']) {
            e.push(format!("{}: недопустимый знак в «{}»", where_, p));
        }
    }
    if n > 500 {
        e.push(format!("{}: больше 500 шаблонов", where_));
    }
    e
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns() {
        let r = IgnoreRules::new(["*.bak", "Сборки/", "док/черновики", "/корень.txt", "# коммент", "node_modules"]);
        assert!(r.is_ignored("a/b/x.bak"));
        assert!(r.is_ignored("сборки/a.exe"));
        assert!(!r.is_ignored("x/Сборки")); // файл с таким именем — не папка
        assert!(r.is_ignored("док/черновики/1.md"));
        assert!(r.is_ignored("док/черновики"));
        assert!(!r.is_ignored("x/док/черновики/1.md"));
        assert!(r.is_ignored("корень.txt"));
        assert!(!r.is_ignored("a/корень.txt"));
        assert!(r.is_ignored("web/node_modules/x/y.js"));
        assert_eq!(r.patterns().len(), 5);
        assert!(!validate(&["**".into()], "x").is_empty());
        assert!(!validate(&["../a".into()], "x").is_empty());
    }
}

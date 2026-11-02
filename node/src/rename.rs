//! (3.13) Английские имена папок и файлов моста. При запуске новой версии прежние русские имена переименовываются
//! (не копируются и не удаляются), настройки с прежними значениями по умолчанию правятся. Папка занята (открыта в
//! Проводнике) или новое имя уже есть — остаётся как было, мост работает по старому пути и попробует при следующем
//! запуске. Своё (папки пространств с другим именем, файлы людей) не трогаем.

use bridge_core::config::NodeConfig;
use std::path::{Path, PathBuf};

/// Папки рядом с мостом, которые он создаёт сам.
const BASE_DIRS: &[(&str, &str)] = &[("агент", "agent"), ("сборки", "builds"), ("приложения", "apps"), ("канал-агента", "agent-channel")];
/// Папки в корзине моста (bridge-data/trash).
const TRASH_DIRS: &[(&str, &str)] = &[("сборки", "builds"), ("приложения", "apps"), ("данные приложений", "app-data")];

/// Что вышло: строки для журнала и поменялись ли настройки (тогда их надо сохранить).
#[derive(Default)]
pub struct Report {
    pub lines: Vec<String>,
    pub config_changed: bool,
}

enum Moved {
    Done,
    Nothing,
    Failed(String),
}

fn mv(from: &Path, to: &Path) -> Moved {
    if !from.exists() {
        return Moved::Nothing;
    }
    if to.exists() {
        return Moved::Failed(format!("«{}» уже есть", to.display()));
    }
    match std::fs::rename(from, to) {
        Ok(()) => Moved::Done,
        Err(e) => Moved::Failed(e.to_string()),
    }
}

/// Переименовать и записать в отчёт. true — по новому имени работать можно (переименовали или старого не было).
fn step(r: &mut Report, from: PathBuf, to: PathBuf) -> bool {
    match mv(&from, &to) {
        Moved::Done => {
            r.lines.push(format!("«{}» → «{}»", from.display(), to.display()));
            true
        }
        Moved::Nothing => true,
        Moved::Failed(e) => {
            r.lines.push(format!("не переименовано «{}» → «{}»: {} — попробую при следующем запуске", from.display(), to.display(), e));
            false
        }
    }
}

/// Значение настройки по умолчанию: прежнее имя → новое, вместе с папкой.
fn setting(r: &mut Report, base: &Path, value: &mut String, old: &str, new: &str) {
    if value.as_str() != old {
        return;
    }
    if step(r, base.join(old), base.join(new)) {
        *value = new.to_string();
        r.config_changed = true;
    }
}

/// Пространства в «Пространства/<id>» → «spaces/<id>».
fn spaces(r: &mut Report, base: &Path, cfg: &mut NodeConfig) {
    const OLD: &str = "Пространства";
    const NEW: &str = "spaces";
    let old_abs = base.join(OLD);
    let strip = |f: &str| -> Option<String> {
        let f = f.replace('\\', "/");
        let abs = old_abs.to_string_lossy().replace('\\', "/");
        if let Some(rest) = f.strip_prefix(&format!("{}/", OLD)) {
            return Some(rest.to_string());
        }
        f.strip_prefix(&format!("{}/", abs)).map(String::from)
    };
    let uses = cfg.shares.iter().any(|s| strip(&s.folder).is_some());
    if !uses && !old_abs.exists() {
        return;
    }
    if !step(r, old_abs.clone(), base.join(NEW)) {
        return;
    }
    for s in cfg.shares.iter_mut() {
        if let Some(rest) = strip(&s.folder) {
            s.folder = format!("{}/{}", NEW, rest);
            r.config_changed = true;
        }
    }
}

/// Внутри папки: прежнее имя → новое (без правки настроек).
fn inside(r: &mut Report, dir: &Path, pairs: &[(&str, &str)]) {
    for (old, new) in pairs {
        step(r, dir.join(old), dir.join(new));
    }
}

fn resolve(base: &Path, p: &str) -> PathBuf {
    let path = Path::new(p);
    if path.is_absolute() || (p.len() > 2 && p.as_bytes()[1] == b':') {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

/// Переименовать прежние русские имена. Зовётся при запуске узла до того, как открыты папки и пространства.
pub fn migrate(base: &Path, data_dir: &Path, cfg: &mut NodeConfig) -> Report {
    let mut r = Report::default();

    // настройки с прежними значениями по умолчанию
    setting(&mut r, base, &mut cfg.mailbox.outgoing, "отправка", "outbox");
    setting(&mut r, base, &mut cfg.mailbox.incoming, "приём", "inbox");
    setting(&mut r, base, &mut cfg.mounts.root, "общие", "shared");
    setting(&mut r, base, &mut cfg.mobile.outbox, "на телефон", "to-phone");
    spaces(&mut r, base, cfg);
    for rc in cfg.runners.values_mut() {
        if rc.output_dir == "_команды" {
            // журналы команд лежат в пространствах; новые пойдут в «_commands», прежние остаются где были
            rc.output_dir = "_commands".into();
            r.config_changed = true;
        }
    }

    // папки, которые мост создаёт сам
    inside(&mut r, base, BASE_DIRS);
    inside(&mut r, &data_dir.join("trash"), TRASH_DIRS);

    // внутри отправки и приёма
    let outbox = resolve(base, &cfg.mailbox.outgoing);
    inside(&mut r, &outbox, &[("_отменено", "_cancelled"), ("_КАК-ОТПРАВИТЬ.txt", "_HOW-TO-SEND.txt")]);
    let inbox = resolve(base, &cfg.mailbox.incoming);
    inside(&mut r, &inbox, &[("ассистент", "assistant")]);

    // свои правки в папках соседей: «общие/<сосед>/<папка>/_мои-правки» → «_my-edits»
    let shared = resolve(base, &cfg.mounts.root);
    for peer in subdirs(&shared) {
        for mount in subdirs(&peer) {
            inside(&mut r, &mount, &[("_мои-правки", "_my-edits")]);
        }
    }

    // снимки файлового интерфейса пространств: records/_без-типа → records/_untyped
    for s in &cfg.shares {
        let root = resolve(base, if s.folder.trim().is_empty() { "" } else { s.folder.trim() });
        inside(&mut r, &root.join(bridge_core::SERVICE_DIR).join("records"), &[("_без-типа", "_untyped")]);
    }

    // указатель каналов efc: в нём прежние имена и пути — каналы запишут себя заново при открытии
    let index = base.join(efc_core::model::INDEX_FILE);
    if std::fs::read_to_string(&index).map(|t| t.contains("канал-агента") || t.contains("Пространства") || t.contains("пространство-")).unwrap_or(false) {
        let _ = bridge_core::write_atomic(&index, "[]\n");
    }

    r
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renames_defaults_and_keeps_custom() {
        let base = std::env::temp_dir().join(format!("bridge-rename-{}", uuid::Uuid::new_v4().simple()));
        let data = base.join("bridge-data");
        for d in ["отправка/_отменено", "приём/ассистент", "общие/peer/docs/_мои-правки", "Пространства/feedback/_bridge/records/_без-типа", "агент", "сборки/game", "приложения/tv", "канал-агента", "мои-пространства/x", "bridge-data/trash/данные приложений"] {
            std::fs::create_dir_all(base.join(d)).unwrap();
        }
        std::fs::write(base.join("отправка/_КАК-ОТПРАВИТЬ.txt"), "x").unwrap();
        std::fs::write(base.join("сборки/game/a.txt"), "a").unwrap();
        let mut cfg = NodeConfig {
            mailbox: bridge_core::config::MailboxConfig { outgoing: "отправка".into(), incoming: "приём".into(), ..Default::default() },
            ..Default::default()
        };
        cfg.mounts.root = "общие".into();
        cfg.mobile.outbox = "на телефон".into();
        cfg.shares = vec![
            bridge_core::config::ShareConfig { id: "feedback".into(), folder: "Пространства/feedback".into(), ..Default::default() },
            bridge_core::config::ShareConfig { id: "x".into(), folder: "мои-пространства/x".into(), ..Default::default() },
        ];
        let r = migrate(&base, &data, &mut cfg);
        assert!(r.config_changed);
        assert_eq!(cfg.mailbox.outgoing, "outbox");
        assert_eq!(cfg.mailbox.incoming, "inbox");
        assert_eq!(cfg.mounts.root, "shared");
        assert_eq!(cfg.mobile.outbox, "to-phone");
        assert_eq!(cfg.shares[0].folder, "spaces/feedback");
        assert_eq!(cfg.shares[1].folder, "мои-пространства/x"); // своё не трогаем
        for p in ["outbox/_cancelled", "outbox/_HOW-TO-SEND.txt", "inbox/assistant", "shared/peer/docs/_my-edits", "spaces/feedback/_bridge/records/_untyped", "agent", "builds/game/a.txt", "apps/tv", "agent-channel", "мои-пространства/x", "bridge-data/trash/app-data"] {
            assert!(base.join(p).exists(), "{}", p);
        }
        assert!(!base.join("отправка").exists() && !base.join("Пространства").exists());
        // второй запуск — ничего не делает
        let r2 = migrate(&base, &data, &mut cfg);
        assert!(!r2.config_changed && r2.lines.is_empty(), "{:?}", r2.lines);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn busy_target_keeps_old_name() {
        let base = std::env::temp_dir().join(format!("bridge-rename-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(base.join("приём/a")).unwrap();
        std::fs::create_dir_all(base.join("inbox/b")).unwrap();
        let mut cfg = NodeConfig::default();
        cfg.mailbox.incoming = "приём".into();
        let r = migrate(&base, &base.join("bridge-data"), &mut cfg);
        assert_eq!(cfg.mailbox.incoming, "приём"); // обе есть — работаем по старому пути
        assert!(base.join("приём/a").exists() && base.join("inbox/b").exists());
        assert!(r.lines.iter().any(|l| l.contains("не переименовано")));
        let _ = std::fs::remove_dir_all(&base);
    }
}

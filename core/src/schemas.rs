//! Схемы DTO пространства: «_schemas/<имя>@<версия>.json» (подмножество JSON Schema):
//! type (в т. ч. список), properties, required, enum, items, additionalProperties (bool), minLength/maxLength,
//! minimum/maximum, minItems/maxItems.

use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Clone, Debug, Default)]
pub struct SchemaSet {
    schemas: BTreeMap<String, Value>,
}

/// «имя@версия»: имя — латиница, цифры, «.», «-», «_»; версия — целое.
pub fn is_schema_id(s: &str) -> bool {
    match s.rfind('@') {
        Some(i) if i > 0 && i < s.len() - 1 => {
            s[..i].chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_') && s[i + 1..].chars().all(|c| c.is_ascii_digit())
        }
        _ => false,
    }
}

impl SchemaSet {
    pub fn load(share_root: &Path, errors: &mut Vec<String>) -> SchemaSet {
        let mut set = SchemaSet::default();
        let dir = share_root.join(crate::SCHEMAS_DIR);
        let Ok(rd) = std::fs::read_dir(&dir) else { return set };
        let mut files: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().map(|x| x == "json").unwrap_or(false)).collect();
        files.sort();
        for f in files {
            let fname = f.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
            let name = f.file_stem().and_then(|n| n.to_str()).unwrap_or("").to_string();
            if !is_schema_id(&name) {
                errors.push(format!("{}: имя файла — «имя@версия.json»", fname));
                continue;
            }
            match std::fs::read_to_string(&f).map_err(|e| e.to_string()).and_then(|t| serde_json::from_str::<Value>(t.trim_start_matches('\u{feff}')).map_err(|e| e.to_string())) {
                Ok(v @ Value::Object(_)) => {
                    set.schemas.insert(name.to_lowercase(), v);
                }
                Ok(_) => errors.push(format!("{}: не объект JSON", fname)),
                Err(e) => errors.push(format!("{}: {}", fname, e)),
            }
        }
        set
    }

    pub fn ids(&self) -> Vec<String> {
        self.schemas.keys().cloned().collect()
    }

    pub fn get(&self, id: &str) -> Option<&Value> {
        self.schemas.get(&id.to_lowercase())
    }

    /// Проверить данные по схеме. Пусто — всё верно; нет схемы — ошибка.
    pub fn validate(&self, schema_id: &str, data: &Value) -> Vec<String> {
        let mut errors = Vec::new();
        match self.get(schema_id) {
            None => errors.push(format!("нет схемы «{}» в {}/", schema_id, crate::SCHEMAS_DIR)),
            Some(s) => check(s, Some(data), "$", &mut errors),
        }
        errors
    }
}

fn is(t: &str, v: Option<&Value>) -> bool {
    match t {
        "null" => v.map(|v| v.is_null()).unwrap_or(true),
        "object" => matches!(v, Some(Value::Object(_))),
        "array" => matches!(v, Some(Value::Array(_))),
        "string" => matches!(v, Some(Value::String(_))),
        "boolean" => matches!(v, Some(Value::Bool(_))),
        "number" => matches!(v, Some(Value::Number(_))),
        "integer" => match v {
            Some(Value::Number(n)) => n.as_f64().map(|d| (d % 1.0).abs() < 1e-12).unwrap_or(false),
            _ => false,
        },
        _ => true,
    }
}

fn num(v: Option<&Value>) -> Option<f64> {
    v.and_then(|v| v.as_f64())
}

fn check(schema: &Value, v: Option<&Value>, at: &str, errors: &mut Vec<String>) {
    if let Some(tn) = schema.get("type") {
        let types: Vec<String> = match tn {
            Value::Array(a) => a.iter().filter_map(|x| x.as_str().map(String::from)).collect(),
            Value::String(s) => vec![s.clone()],
            _ => vec![],
        };
        if !types.iter().any(|t| is(t, v)) {
            errors.push(format!("{}: ожидался тип {}", at, types.join("|")));
            return;
        }
    }
    if let Some(Value::Array(en)) = schema.get("enum") {
        let val = v.cloned().unwrap_or(Value::Null);
        if !en.iter().any(|x| *x == val) {
            errors.push(format!("{}: значение не из списка {}", at, serde_json::to_string(en).unwrap_or_default()));
        }
    }
    match v {
        Some(Value::String(s)) => {
            let len = s.encode_utf16().count() as f64;
            if let Some(min) = num(schema.get("minLength")) {
                if len < min {
                    errors.push(format!("{}: короче {}", at, min));
                }
            }
            if let Some(max) = num(schema.get("maxLength")) {
                if len > max {
                    errors.push(format!("{}: длиннее {}", at, max));
                }
            }
        }
        Some(Value::Number(n)) => {
            let d = n.as_f64().unwrap_or(0.0);
            if let Some(mn) = num(schema.get("minimum")) {
                if d < mn {
                    errors.push(format!("{}: меньше {}", at, mn));
                }
            }
            if let Some(mx) = num(schema.get("maximum")) {
                if d > mx {
                    errors.push(format!("{}: больше {}", at, mx));
                }
            }
        }
        Some(Value::Object(obj)) => {
            if let Some(Value::Array(req)) = schema.get("required") {
                for r in req.iter().filter_map(|r| r.as_str()) {
                    if !obj.contains_key(r) {
                        errors.push(format!("{}.{}: обязательное поле", at, r));
                    }
                }
            }
            let props = schema.get("properties").and_then(|p| p.as_object());
            for (k, val) in obj {
                match props.and_then(|p| p.get(k)) {
                    Some(ps @ Value::Object(_)) => check(ps, Some(val), &format!("{}.{}", at, k), errors),
                    _ => {
                        if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
                            errors.push(format!("{}.{}: поле не описано в схеме", at, k));
                        }
                    }
                }
            }
        }
        Some(Value::Array(list)) => {
            if let Some(mi) = num(schema.get("minItems")) {
                if (list.len() as f64) < mi {
                    errors.push(format!("{}: элементов меньше {}", at, mi));
                }
            }
            if let Some(ma) = num(schema.get("maxItems")) {
                if (list.len() as f64) > ma {
                    errors.push(format!("{}: элементов больше {}", at, ma));
                }
            }
            if let Some(items @ Value::Object(_)) = schema.get("items") {
                for (i, x) in list.iter().enumerate() {
                    check(items, Some(x), &format!("{}[{}]", at, i), errors);
                }
            }
        }
        _ => {}
    }
}

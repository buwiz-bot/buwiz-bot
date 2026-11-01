//! `GET /skills`: the user's own skills (server/src/routes/market.mjs).
//! Node still writes them; this reads the rows as they stand.

use serde_json::{Map, Value};

use super::{sql_json, Answer, Leaf};
use crate::js;

pub fn get(leaf: &Leaf) -> Answer {
    // `SELECT *` as Node runs it, so rows with the same created_at keep
    // Node's order
    let rows = leaf.db(|db| {
        let mut stmt = db.prepare("SELECT * FROM skills ORDER BY created_at")?;
        let names: Vec<String> = stmt.column_names().into_iter().map(str::to_owned).collect();
        let col = |k: &str| names.iter().position(|n| n == k);
        let cols: Vec<Option<usize>> = ["id", "name", "description", "body", "enabled"].iter().map(|k| col(k)).collect();
        let rows = stmt.query_map([], |r| {
            let mut m = Map::new();
            for (k, at) in ["id", "name", "description", "body"].iter().zip(&cols) {
                // a column that isn't there reads as undefined, which JSON drops
                if let Some(at) = at {
                    m.insert((*k).to_owned(), sql_json(r.get_ref(*at)?));
                }
            }
            let enabled = match cols[4] {
                Some(at) => js::truthy(&sql_json(r.get_ref(at)?)),
                None => false,
            };
            m.insert("enabled".to_owned(), Value::Bool(enabled));
            Ok(Value::Object(m))
        })?;
        rows.collect::<rusqlite::Result<Vec<Value>>>()
    });
    match rows {
        Ok(list) => Answer::json(200, format!("{{\"skills\":{}}}", js::stringify(&Value::Array(list)))),
        Err(e) => Answer::thrown(&e),
    }
}

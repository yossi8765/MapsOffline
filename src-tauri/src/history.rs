//! SQLite-backed history & favorites.
use std::path::Path;

use anyhow::Context;
use rusqlite::{params, Connection, Row};

use crate::types::{Favorite, HistoryItem, HistoryKind};

const MAX_HISTORY: i64 = 2000;
const DEDUPE_WINDOW: i64 = 50;

pub struct History {
    conn: Connection,
}

fn kind_str(k: HistoryKind) -> &'static str {
    match k {
        HistoryKind::Search => "search",
        HistoryKind::Route => "route",
    }
}

fn kind_from(s: &str) -> HistoryKind {
    if s == "route" {
        HistoryKind::Route
    } else {
        HistoryKind::Search
    }
}

fn now_iso() -> String {
    chrono::Utc::now()
        .with_timezone(&chrono_tz::Asia::Jerusalem)
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
}

pub fn open(path: &Path) -> anyhow::Result<History> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    let conn = Connection::open(path).with_context(|| format!("open history db {}", path.display()))?;
    // journal_mode returns a row, so use query_row.
    let _: String = conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
    conn.execute_batch(
        "PRAGMA synchronous=NORMAL;
         CREATE TABLE IF NOT EXISTS history(
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            kind TEXT NOT NULL,
            created_iso TEXT NOT NULL,
            label TEXT NOT NULL,
            payload TEXT NOT NULL DEFAULT 'null'
         );
         CREATE INDEX IF NOT EXISTS idx_history_kind ON history(kind, id);
         CREATE TABLE IF NOT EXISTS favorites(
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            icon TEXT NOT NULL DEFAULT 'star',
            lon REAL NOT NULL,
            lat REAL NOT NULL,
            address TEXT,
            created_iso TEXT NOT NULL
         );",
    )?;
    // favorites gained a kind + data column; older databases get them here
    let cols: Vec<String> = conn.prepare("PRAGMA table_info(favorites)")?.query_map([], |r| r.get::<_, String>(1))?.collect::<Result<_, _>>()?;
    if !cols.iter().any(|c| c == "kind") {
        conn.execute_batch("ALTER TABLE favorites ADD COLUMN kind TEXT NOT NULL DEFAULT 'address'; ALTER TABLE favorites ADD COLUMN data TEXT;")?;
    }
    Ok(History { conn })
}

fn row_item(r: &Row) -> rusqlite::Result<HistoryItem> {
    let kind: String = r.get(1)?;
    let payload: String = r.get(4)?;
    Ok(HistoryItem {
        id: r.get(0)?,
        kind: kind_from(&kind),
        created_iso: r.get(2)?,
        label: r.get(3)?,
        payload: serde_json::from_str(&payload).unwrap_or(serde_json::Value::Null),
    })
}

fn row_fav(r: &Row) -> rusqlite::Result<Favorite> {
    Ok(Favorite {
        id: r.get(0)?,
        name: r.get(1)?,
        icon: r.get(2)?,
        lon: r.get(3)?,
        lat: r.get(4)?,
        address: r.get(5)?,
        created_iso: r.get(6)?,
        kind: r.get(7)?,
        data: r.get::<_, Option<String>>(8)?.and_then(|s| serde_json::from_str(&s).ok()),
    })
}

impl History {
    pub fn list(&self, kind: Option<HistoryKind>, limit: usize, filter: Option<&str>) -> anyhow::Result<Vec<HistoryItem>> {
        let mut sql = String::from("SELECT id, kind, created_iso, label, payload FROM history WHERE 1=1");
        let mut args: Vec<rusqlite::types::Value> = Vec::new();
        if let Some(k) = kind {
            sql.push_str(" AND kind = ?");
            args.push(kind_str(k).to_string().into());
        }
        if let Some(f) = filter.map(str::trim).filter(|f| !f.is_empty()) {
            let esc = f.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
            sql.push_str(" AND label LIKE ? ESCAPE '\\'");
            args.push(format!("%{esc}%").into());
        }
        sql.push_str(" ORDER BY id DESC LIMIT ?");
        args.push((limit as i64).into());
        let mut st = self.conn.prepare(&sql)?;
        let rows = st.query_map(rusqlite::params_from_iter(args), row_item)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn add(&self, kind: HistoryKind, label: &str, payload: serde_json::Value) -> anyhow::Result<HistoryItem> {
        let k = kind_str(kind);
        // Move-to-top semantics: drop an identical recent entry.
        self.conn.execute(
            "DELETE FROM history WHERE kind = ?1 AND label = ?2 AND id IN
               (SELECT id FROM history ORDER BY id DESC LIMIT ?3)",
            params![k, label, DEDUPE_WINDOW],
        )?;
        let created = now_iso();
        let payload_s = serde_json::to_string(&payload)?;
        self.conn.execute(
            "INSERT INTO history(kind, created_iso, label, payload) VALUES (?1, ?2, ?3, ?4)",
            params![k, created, label, payload_s],
        )?;
        let id = self.conn.last_insert_rowid();
        self.conn.execute(
            "DELETE FROM history WHERE id <= (SELECT id FROM history ORDER BY id DESC LIMIT 1 OFFSET ?1)",
            params![MAX_HISTORY],
        )?;
        Ok(HistoryItem { id, kind, created_iso: created, label: label.to_string(), payload })
    }

    pub fn delete(&self, id: i64) -> anyhow::Result<()> {
        self.conn.execute("DELETE FROM history WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn clear(&self, kind: Option<HistoryKind>) -> anyhow::Result<()> {
        match kind {
            Some(k) => self.conn.execute("DELETE FROM history WHERE kind = ?1", params![kind_str(k)])?,
            None => self.conn.execute("DELETE FROM history", [])?,
        };
        Ok(())
    }

    pub fn favorites(&self) -> anyhow::Result<Vec<Favorite>> {
        let mut st = self
            .conn
            .prepare("SELECT id, name, icon, lon, lat, address, created_iso, kind, data FROM favorites ORDER BY id")?;
        let rows = st.query_map([], row_fav)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Adds a favorite, or returns the existing one for the same place (same kind within ~10 cm)
    /// or the same line (same data), so a second click cannot create a duplicate.
    pub fn favorite_add(&self, name: &str, icon: &str, lon: f64, lat: f64, address: Option<&str>, kind: &str, data: Option<&serde_json::Value>) -> anyhow::Result<Favorite> {
        if let Some(existing) = self.favorites()?.into_iter().find(|f| {
            f.kind == kind && if kind == "line" { f.data.as_ref() == data } else { (f.lon - lon).abs() < 1e-6 && (f.lat - lat).abs() < 1e-6 }
        }) {
            return Ok(existing);
        }
        let created = now_iso();
        let data_s = data.map(|d| d.to_string());
        self.conn.execute(
            "INSERT INTO favorites(name, icon, lon, lat, address, created_iso, kind, data) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![name, icon, lon, lat, address, created, kind, data_s],
        )?;
        Ok(Favorite {
            id: self.conn.last_insert_rowid(),
            name: name.to_string(),
            icon: icon.to_string(),
            lon,
            lat,
            address: address.map(str::to_string),
            created_iso: created,
            kind: kind.to_string(),
            data: data.cloned(),
        })
    }

    pub fn favorite_update(&self, id: i64, name: &str, icon: &str) -> anyhow::Result<()> {
        self.conn
            .execute("UPDATE favorites SET name = ?2, icon = ?3 WHERE id = ?1", params![id, name, icon])?;
        Ok(())
    }

    pub fn favorite_delete(&self, id: i64) -> anyhow::Result<()> {
        self.conn.execute("DELETE FROM favorites WHERE id = ?1", params![id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tmp(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("mapsoffline_hist_{}_{}.db", name, std::process::id()));
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{}", p.display(), ext));
        }
        p
    }

    #[test]
    fn history_flow() {
        let p = tmp("flow");
        let h = open(&p).unwrap();
        let a = h.add(HistoryKind::Search, "הרצל 12", json!({"x": 1})).unwrap();
        let _b = h.add(HistoryKind::Route, "A → B", json!({})).unwrap();
        let c = h.add(HistoryKind::Search, "דיזנגוף", json!(null)).unwrap();
        let a2 = h.add(HistoryKind::Search, "הרצל 12", json!({"x": 2})).unwrap();
        assert!(a2.id > a.id && a2.id > c.id);
        let all = h.list(None, 10, None).unwrap();
        assert_eq!(all.len(), 3); // deduped
        assert_eq!(all[0].label, "הרצל 12");
        assert_eq!(all[0].payload["x"], 2);
        assert_eq!(h.list(Some(HistoryKind::Route), 10, None).unwrap().len(), 1);
        assert_eq!(h.list(None, 10, Some("זנג")).unwrap().len(), 1);
        assert_eq!(h.list(None, 10, Some("100%")).unwrap().len(), 0);
        h.delete(c.id).unwrap();
        assert_eq!(h.list(None, 10, None).unwrap().len(), 2);
        h.clear(Some(HistoryKind::Search)).unwrap();
        assert_eq!(h.list(None, 10, None).unwrap().len(), 1);
        h.clear(None).unwrap();
        assert!(h.list(None, 10, None).unwrap().is_empty());
        drop(h);
        // reopen keeps schema
        let h = open(&p).unwrap();
        assert!(h.list(None, 10, None).unwrap().is_empty());
    }

    #[test]
    fn cap_and_favorites() {
        let p = tmp("cap");
        let h = open(&p).unwrap();
        for i in 0..2010 {
            h.add(HistoryKind::Search, &format!("q{i}"), json!(null)).unwrap();
        }
        let all = h.list(None, 5000, None).unwrap();
        assert_eq!(all.len() as i64, MAX_HISTORY);
        assert_eq!(all[0].label, "q2009");

        let f = h.favorite_add("בית", "home", 34.78, 32.08, Some("הרצל 1"), "address", None).unwrap();
        h.favorite_add("עבודה", "briefcase", 34.8, 32.1, None, "address", None).unwrap();
        // same spot again = same row, not a duplicate
        assert_eq!(h.favorite_add("בית שוב", "star", 34.78, 32.08, None, "address", None).unwrap().id, f.id);
        let line = serde_json::json!({"routeShortName": "66", "agency": "דן", "headsign": "פתח תקווה"});
        let l1 = h.favorite_add("קו 66", "bus", 34.8, 32.1, None, "line", Some(&line)).unwrap();
        assert_eq!(h.favorite_add("קו 66", "bus", 34.9, 32.2, None, "line", Some(&line)).unwrap().id, l1.id);
        assert_eq!(h.favorites().unwrap().len(), 3);
        h.favorite_delete(l1.id).unwrap();
        h.favorite_update(f.id, "הבית", "star").unwrap();
        let favs = h.favorites().unwrap();
        assert_eq!(favs.len(), 2);
        assert_eq!(favs[0].name, "הבית");
        assert_eq!(favs[0].icon, "star");
        h.favorite_delete(f.id).unwrap();
        assert_eq!(h.favorites().unwrap().len(), 1);
    }
}

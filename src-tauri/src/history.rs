//! SQLite-backed history & favorites.
use std::path::Path;

use anyhow::Context;
use rusqlite::{params, Connection, Row};

use crate::types::{Favorite, FavoritePatch, HistoryItem, HistoryKind};

const MAX_HISTORY: i64 = 2000;
const DEDUPE_WINDOW: i64 = 50;
/// A history payload larger than this is stored without its bulky parts (geometries).
const MAX_PAYLOAD_BYTES: usize = 16 * 1024;

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
    // folders, colours and notes (2026-11 improvements)
    if !cols.iter().any(|c| c == "grp") {
        conn.execute_batch("ALTER TABLE favorites ADD COLUMN grp TEXT; ALTER TABLE favorites ADD COLUMN color TEXT; ALTER TABLE favorites ADD COLUMN note TEXT;")?;
    }
    Ok(History { conn })
}

/// Drop geometries (and, if still too big, everything but the request) from a payload so the
/// history table stays small: a route is recomputed from its request anyway.
fn slim_payload(mut v: serde_json::Value) -> serde_json::Value {
    fn strip(v: &mut serde_json::Value) {
        match v {
            serde_json::Value::Object(m) => {
                m.remove("geometry");
                m.remove("legs");
                m.values_mut().for_each(strip);
            }
            serde_json::Value::Array(a) => a.iter_mut().for_each(strip),
            _ => {}
        }
    }
    if v.to_string().len() <= MAX_PAYLOAD_BYTES {
        return v;
    }
    strip(&mut v);
    if v.to_string().len() > MAX_PAYLOAD_BYTES {
        if let Some(req) = v.get("request").cloned() {
            return serde_json::json!({ "request": req });
        }
        return serde_json::Value::Null;
    }
    v
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
        group: r.get(9)?,
        color: r.get(10)?,
        note: r.get(11)?,
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
        let payload = slim_payload(payload);
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
            .prepare("SELECT id, name, icon, lon, lat, address, created_iso, kind, data, grp, color, note FROM favorites ORDER BY id")?;
        let rows = st.query_map([], row_fav)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Adds a favorite, or returns the existing one for the same place (same kind within ~10 cm)
    /// or the same line (same data), so a second click cannot create a duplicate.
    pub fn favorite_add(&self, name: &str, icon: &str, lon: f64, lat: f64, address: Option<&str>, kind: &str, data: Option<&serde_json::Value>) -> anyhow::Result<Favorite> {
        self.favorite_add_ext(name, icon, lon, lat, address, kind, data, None, None, None)
    }

    /// [`favorite_add`] with a folder, colour and note. Lines and routes are the same favorite
    /// when their data is equal; other kinds when they are at the same spot.
    pub fn favorite_add_ext(
        &self,
        name: &str,
        icon: &str,
        lon: f64,
        lat: f64,
        address: Option<&str>,
        kind: &str,
        data: Option<&serde_json::Value>,
        group: Option<&str>,
        color: Option<&str>,
        note: Option<&str>,
    ) -> anyhow::Result<Favorite> {
        if let Some(existing) = self.favorites()?.into_iter().find(|f| {
            f.kind == kind
                && if kind == "line" || kind == "route" { f.data.as_ref() == data } else { (f.lon - lon).abs() < 1e-6 && (f.lat - lat).abs() < 1e-6 }
        }) {
            return Ok(existing);
        }
        let created = now_iso();
        let data_s = data.map(|d| d.to_string());
        let clean = |v: Option<&str>| v.map(str::trim).filter(|v| !v.is_empty()).map(str::to_string);
        let (group, color, note) = (clean(group), clean(color), clean(note));
        if let Some(c) = &color {
            check_color(c)?;
        }
        self.conn.execute(
            "INSERT INTO favorites(name, icon, lon, lat, address, created_iso, kind, data, grp, color, note) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![name, icon, lon, lat, address, created, kind, data_s, group, color, note],
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
            group,
            color,
            note,
        })
    }

    pub fn favorite_get(&self, id: i64) -> anyhow::Result<Favorite> {
        self.favorites()?.into_iter().find(|f| f.id == id).ok_or_else(|| anyhow::anyhow!("favorite {id} not found"))
    }

    /// Change some fields of a favorite (absent = unchanged, explicit null = cleared).
    pub fn favorite_patch(&self, id: i64, p: &FavoritePatch) -> anyhow::Result<Favorite> {
        let tx = self.conn.unchecked_transaction()?;
        if let Some(n) = &p.name {
            tx.execute("UPDATE favorites SET name = ?2 WHERE id = ?1", params![id, n])?;
        }
        if let Some(i) = &p.icon {
            tx.execute("UPDATE favorites SET icon = ?2 WHERE id = ?1", params![id, i])?;
        }
        let opt = |v: &Option<String>| v.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
        if let Some(g) = &p.group {
            tx.execute("UPDATE favorites SET grp = ?2 WHERE id = ?1", params![id, opt(g)])?;
        }
        if let Some(c) = &p.color {
            let c = opt(c);
            if let Some(c) = &c {
                check_color(c)?;
            }
            tx.execute("UPDATE favorites SET color = ?2 WHERE id = ?1", params![id, c])?;
        }
        if let Some(n) = &p.note {
            tx.execute("UPDATE favorites SET note = ?2 WHERE id = ?1", params![id, opt(n)])?;
        }
        if let Some(a) = &p.address {
            tx.execute("UPDATE favorites SET address = ?2 WHERE id = ?1", params![id, opt(a)])?;
        }
        if let Some(d) = &p.data {
            tx.execute("UPDATE favorites SET data = ?2 WHERE id = ?1", params![id, d.to_string()])?;
        }
        tx.commit()?;
        self.favorite_get(id)
    }

    /// Folder names in use, alphabetical.
    pub fn favorite_groups(&self) -> anyhow::Result<Vec<String>> {
        let mut st = self.conn.prepare("SELECT DISTINCT grp FROM favorites WHERE grp IS NOT NULL AND grp <> '' ORDER BY grp")?;
        let rows = st.query_map([], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Fold the write-ahead log into the database (called when the app exits, so the folder does
    /// not keep a WAL file larger than the database).
    pub fn checkpoint(&self) {
        if let Err(e) = self.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);") {
            log::warn!("history checkpoint: {e}");
        }
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

fn check_color(c: &str) -> anyhow::Result<()> {
    let ok = c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|ch| ch.is_ascii_hexdigit());
    anyhow::ensure!(ok, "INVALID_COLOR: {c}");
    Ok(())
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

    #[test]
    fn favorite_groups_patch_and_routes() {
        let p = tmp("groups");
        let h = open(&p).unwrap();
        let a = h.favorite_add_ext("מעיין", "star", 35.0, 31.8, None, "place", None, Some("טיולים"), Some("#2266aa"), Some("מים קרים")).unwrap();
        assert_eq!(a.group.as_deref(), Some("טיולים"));
        let req = json!({"waypoints": [{"lon": 34.7, "lat": 32.0}, {"lon": 35.2, "lat": 31.7}], "mode": "car"});
        let r1 = h.favorite_add_ext("לעבודה", "route", 34.7, 32.0, None, "route", Some(&req), None, None, None).unwrap();
        // same request = same favorite even from another start point field
        assert_eq!(h.favorite_add_ext("x", "route", 0.0, 0.0, None, "route", Some(&req), None, None, None).unwrap().id, r1.id);
        assert!(h.favorite_add_ext("bad", "star", 1.0, 1.0, None, "place", None, None, Some("red"), None).is_err());
        let patched = h
            .favorite_patch(a.id, &serde_json::from_value::<FavoritePatch>(json!({"group": null, "note": "קר מאוד", "color": "#ff0000"})).unwrap())
            .unwrap();
        assert_eq!(patched.group, None);
        assert_eq!(patched.note.as_deref(), Some("קר מאוד"));
        assert_eq!(patched.color.as_deref(), Some("#ff0000"));
        assert_eq!(patched.name, "מעיין"); // untouched
        h.favorite_patch(r1.id, &serde_json::from_value::<FavoritePatch>(json!({"group": "נסיעות"})).unwrap()).unwrap();
        assert_eq!(h.favorite_groups().unwrap(), vec!["נסיעות".to_string()]);
        h.checkpoint();
    }

    #[test]
    fn big_payload_is_slimmed() {
        let geometry: Vec<[f64; 2]> = (0..5000).map(|i| [34.0 + i as f64 * 1e-4, 32.0]).collect();
        let v = json!({"request": {"mode": "car"}, "summary": {"distanceM": 5}, "route": {"geometry": geometry}});
        let s = slim_payload(v);
        assert!(s.to_string().len() < MAX_PAYLOAD_BYTES);
        assert_eq!(s["request"]["mode"], "car");
        assert_eq!(s["summary"]["distanceM"], 5);
    }
}

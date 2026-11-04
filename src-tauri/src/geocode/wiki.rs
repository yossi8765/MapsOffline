//! Offline Wikipedia summaries for the places and points of interest that OSM links to Wikipedia
//! (`wikipedia=he:Title`, `wikidata=Q…`). Fetched once while the Israel address index is built
//! (stored in its `wiki` table) and read offline by [`super::Geocoder::wiki`].
//!
//! Politeness: the MediaWiki action API with up to 20 titles per request (one request instead
//! of 20), at most 5 requests per second, a descriptive User-Agent, at most [`MAX_TITLES`]
//! titles. English-tagged features whose Wikidata item has a Hebrew article get the Hebrew
//! summary (Wikidata `wbgetentities`, 50 items per request). Any failure only skips titles.
use std::time::{Duration, Instant};

use rustc_hash::{FxHashMap, FxHashSet};
use serde_json::Value;

pub const MAX_TITLES: usize = 6000;
const BATCH: usize = 20;
const MIN_GAP: Duration = Duration::from_millis(210);
const EXTRACT_CHARS: usize = 700;

/// One stored summary. `tag` is the OSM tag as written ("he:כיכר רבין"), the lookup key.
#[derive(Debug, Clone)]
pub struct WikiRow {
    pub tag: String,
    pub qid: Option<String>,
    pub title: String,
    pub extract: String,
    pub url: String,
    pub lang: String,
}

/// "he:Title_x" → ("he", "Title x"); None for malformed tags.
pub fn split_tag(tag: &str) -> Option<(String, String)> {
    let (lang, title) = tag.split_once(':')?;
    let lang = lang.trim().to_ascii_lowercase();
    let title = title.trim().replace('_', " ");
    (lang.len() >= 2 && lang.len() <= 12 && lang.chars().all(|c| c.is_ascii_lowercase() || c == '-') && !title.is_empty()).then_some((lang, title))
}

/// The key a tag is stored under: language + title with spaces ("he:כיכר רבין").
pub fn key(tag: &str) -> Option<String> {
    split_tag(tag).map(|(l, t)| format!("{l}:{t}"))
}

fn client() -> Option<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .user_agent(format!(
            "MapsOffline/{} (offline maps for Israel; +https://gitlab.com/RecoveryAdvancedFree/MapsOffline)",
            env!("CARGO_PKG_VERSION")
        ))
        .timeout(Duration::from_secs(25))
        .build()
        .ok()
}

struct Pacer(Instant);
impl Pacer {
    fn wait(&mut self) {
        let e = self.0.elapsed();
        if e < MIN_GAP {
            std::thread::sleep(MIN_GAP - e);
        }
        self.0 = Instant::now();
    }
}

/// Shorten an extract to about [`EXTRACT_CHARS`] characters at a sentence end.
fn shorten(s: &str) -> String {
    let s = s.trim();
    if s.chars().count() <= EXTRACT_CHARS {
        return s.to_string();
    }
    let cut: String = s.chars().take(EXTRACT_CHARS).collect();
    match cut.rfind(['.', '!', '?']) {
        Some(i) if i > EXTRACT_CHARS / 3 => cut[..=i].to_string(),
        _ => format!("{}…", cut.trim_end()),
    }
}

/// Hebrew article titles of Wikidata items (qid → title), 50 per request.
fn hewiki_titles(c: &reqwest::blocking::Client, qids: &[String], pace: &mut Pacer) -> FxHashMap<String, String> {
    let mut out = FxHashMap::default();
    for chunk in qids.chunks(50) {
        pace.wait();
        let url = "https://www.wikidata.org/w/api.php";
        let ids = chunk.join("|");
        let r = c.get(url).query(&[("action", "wbgetentities"), ("format", "json"), ("props", "sitelinks"), ("sitefilter", "hewiki"), ("ids", ids.as_str())]).send();
        let Ok(v) = r.and_then(|r| r.error_for_status()).and_then(|r| r.json::<Value>()) else {
            log::warn!("wikidata sitelinks request failed");
            continue;
        };
        if let Some(ents) = v["entities"].as_object() {
            for (q, e) in ents {
                if let Some(t) = e["sitelinks"]["hewiki"]["title"].as_str() {
                    out.insert(q.clone(), t.to_string());
                }
            }
        }
    }
    out
}

/// Summaries (intro, plain text) of `titles` in `lang`: title as asked → (title, extract, url).
fn extracts(c: &reqwest::blocking::Client, lang: &str, titles: &[String], pace: &mut Pacer, report: &dyn Fn(&str), done: &mut usize, total: usize) -> FxHashMap<String, (String, String, String)> {
    let mut out = FxHashMap::default();
    for chunk in titles.chunks(BATCH) {
        pace.wait();
        let url = format!("https://{lang}.wikipedia.org/w/api.php");
        let joined = chunk.join("|");
        let r = c
            .get(&url)
            .query(&[
                ("action", "query"),
                ("format", "json"),
                ("formatversion", "2"),
                ("prop", "extracts|info"),
                ("exintro", "1"),
                ("explaintext", "1"),
                ("exlimit", "20"),
                ("inprop", "url"),
                ("redirects", "1"),
                ("titles", joined.as_str()),
            ])
            .send();
        *done += chunk.len();
        if *done % 400 < BATCH {
            report(&format!("תקצירי ויקיפדיה: {}/{}", (*done).min(total), total));
        }
        let v = match r.and_then(|r| r.error_for_status()).and_then(|r| r.json::<Value>()) {
            Ok(v) => v,
            Err(e) => {
                log::warn!("wikipedia ({lang}) request failed: {e}");
                if e.status().is_some_and(|s| s.as_u16() == 418 || s.as_u16() == 403) {
                    break; // blocked (content filter): stop asking this site
                }
                continue;
            }
        };
        // asked title → final title, through normalisation and redirects
        let mut alias: FxHashMap<String, String> = FxHashMap::default();
        for k in ["normalized", "redirects"] {
            if let Some(a) = v["query"][k].as_array() {
                for x in a {
                    if let (Some(f), Some(t)) = (x["from"].as_str(), x["to"].as_str()) {
                        alias.insert(f.to_string(), t.to_string());
                    }
                }
            }
        }
        let mut pages: FxHashMap<String, (String, String, String)> = FxHashMap::default();
        if let Some(ps) = v["query"]["pages"].as_array() {
            for p in ps {
                let (Some(t), Some(ex)) = (p["title"].as_str(), p["extract"].as_str()) else { continue };
                if ex.trim().is_empty() {
                    continue;
                }
                let url = p["fullurl"].as_str().map(str::to_string).unwrap_or_else(|| format!("https://{lang}.wikipedia.org/wiki/{}", t.replace(' ', "_")));
                pages.insert(t.to_string(), (t.to_string(), shorten(ex), url));
            }
        }
        for asked in chunk {
            let mut t = asked.clone();
            for _ in 0..3 {
                match alias.get(&t) {
                    Some(n) => t = n.clone(),
                    None => break,
                }
            }
            if let Some(p) = pages.get(&t) {
                out.insert(asked.clone(), p.clone());
            }
        }
    }
    out
}

/// Fetch the summaries of `tags` ((wikipedia tag, wikidata id)); Hebrew and English articles.
pub fn fetch(tags: &[(String, Option<String>)], report: &dyn Fn(&str)) -> Vec<WikiRow> {
    let Some(c) = client() else { return Vec::new() };
    let mut pace = Pacer(Instant::now() - MIN_GAP);
    // distinct tags, Hebrew first, capped
    let mut seen: FxHashSet<String> = FxHashSet::default();
    let mut list: Vec<(String, String, String, Option<String>)> = Vec::new(); // key, lang, title, qid
    for (tag, qid) in tags {
        let Some((lang, title)) = split_tag(tag) else { continue };
        let k = format!("{lang}:{title}");
        if seen.insert(k.clone()) {
            list.push((k, lang, title, qid.clone()));
        }
    }
    list.sort_by_key(|x| (x.1 != "he", x.1 != "en"));
    list.retain(|x| x.1 == "he" || x.1 == "en");
    list.truncate(MAX_TITLES);
    // English tags whose item has a Hebrew article → read the Hebrew one
    let en_qids: Vec<String> = list.iter().filter(|x| x.1 == "en").filter_map(|x| x.3.clone()).collect();
    let he_of = if en_qids.is_empty() { FxHashMap::default() } else { hewiki_titles(&c, &en_qids, &mut pace) };
    let mut by_lang: FxHashMap<String, Vec<String>> = FxHashMap::default();
    let mut source: Vec<(String, String)> = Vec::with_capacity(list.len()); // (lang, title) to read per entry
    for (_, lang, title, qid) in &list {
        let (l, t) = match qid.as_ref().and_then(|q| he_of.get(q)) {
            Some(he) if lang == "en" => ("he".to_string(), he.clone()),
            _ => (lang.clone(), title.clone()),
        };
        by_lang.entry(l.clone()).or_default().push(t.clone());
        source.push((l, t));
    }
    let total: usize = by_lang.values().map(Vec::len).sum();
    report(&format!("תקצירי ויקיפדיה: 0/{total}"));
    let mut done = 0usize;
    let mut got: FxHashMap<(String, String), (String, String, String)> = FxHashMap::default();
    for (lang, mut titles) in by_lang {
        titles.sort();
        titles.dedup();
        for (t, v) in extracts(&c, &lang, &titles, &mut pace, report, &mut done, total) {
            got.insert((lang.clone(), t), v);
        }
    }
    let mut out = Vec::new();
    for ((key, _, _, qid), (l, t)) in list.into_iter().zip(source) {
        if let Some((title, extract, url)) = got.get(&(l.clone(), t)) {
            out.push(WikiRow { tag: key, qid, title: title.clone(), extract: extract.clone(), url: url.clone(), lang: l });
        }
    }
    log::info!("wikipedia: {} of {total} summaries", out.len());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags() {
        assert_eq!(split_tag("he:כיכר_רבין"), Some(("he".into(), "כיכר רבין".into())));
        assert_eq!(key("en:Rabin Square").as_deref(), Some("en:Rabin Square"));
        assert_eq!(split_tag("Rabin Square"), None);
        let long = "א. ".repeat(400);
        assert!(shorten(&long).chars().count() <= EXTRACT_CHARS + 1);
    }

    /// Network: `cargo test --features cli --lib wiki::tests::live -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live() {
        let rows = fetch(&[("he:כיכר רבין".into(), None), ("en:Azrieli Center".into(), Some("Q1145963".into()))], &|m| println!("{m}"));
        for r in &rows {
            println!("{} [{}] {} – {}", r.tag, r.lang, r.title, r.extract.chars().take(80).collect::<String>());
        }
        assert!(!rows.is_empty());
    }
}

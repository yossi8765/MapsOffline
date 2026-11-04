//! Details of a point of interest or landmark beyond its name and category, stored as compact
//! JSON in `pois.info` / `places.info` (builder revision 4) and returned in `GeoExtra`.
use serde::{Deserialize, Serialize};

use crate::types::GeoExtra;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PoiInfo {
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "p")]
    pub phone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "w")]
    pub website: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "a")]
    pub address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "b")]
    pub brand: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "o")]
    pub operator: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "wp")]
    pub wikipedia: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "wd")]
    pub wikidata: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "x")]
    pub extra: Option<String>,
}

impl PoiInfo {
    pub fn is_empty(&self) -> bool {
        *self == PoiInfo::default()
    }

    pub fn to_json(&self) -> Option<String> {
        (!self.is_empty()).then(|| serde_json::to_string(self).unwrap_or_default())
    }

    pub fn from_json(s: Option<&str>) -> Option<PoiInfo> {
        serde_json::from_str(s?).ok()
    }

    /// Fill the empty fields of `self` from `o` (an open-data register enriching an OSM point).
    pub fn fill_from(&mut self, o: &PoiInfo) {
        macro_rules! fill {
            ($($f:ident),*) => { $( if self.$f.is_none() { self.$f = o.$f.clone(); } )* };
        }
        fill!(phone, website, address, brand, operator, wikipedia, wikidata, extra);
    }

    /// Copy into the result details.
    pub fn apply(&self, e: &mut GeoExtra) {
        e.phone = self.phone.clone();
        e.website = self.website.clone();
        e.address = self.address.clone();
        e.brand = self.brand.clone();
        e.operator = self.operator.clone();
        e.wikipedia = self.wikipedia.clone();
        e.wikidata = self.wikidata.clone();
        e.extra_info = self.extra.clone();
    }
}

/// A phone number as the national digits ("+972 3-612-3456" → "036123456"); None when it is not
/// one. Used both for the search index and for typed queries.
pub fn phone_key(s: &str) -> Option<String> {
    let first = s.split([';', ',']).next()?.trim();
    if first.chars().any(|c| c.is_alphabetic()) {
        return None;
    }
    let mut d: String = first.chars().filter(|c| c.is_ascii_digit()).collect();
    if first.trim_start().starts_with('+') || d.starts_with("972") {
        d = d.strip_prefix("972").map(|r| format!("0{}", r.trim_start_matches('0'))).unwrap_or(d);
    }
    // *2345 / 1-700 numbers and the like are fine too
    (5..=11).contains(&d.len()).then_some(d)
}

/// The first value of a `;`-list, trimmed, limited in length.
pub fn first_value(v: &str, max: usize) -> Option<String> {
    let s = v.split(';').next()?.trim();
    (!s.is_empty()).then(|| s.chars().take(max).collect())
}

/// Hebrew line about a charging station from its OSM tags: capacity and socket types.
pub fn charging_line(capacity: &str, sockets: &[&str]) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Ok(n) = capacity.trim().parse::<u32>() {
        if n > 0 {
            parts.push(if n == 1 { "עמדה אחת".to_string() } else { format!("{n} עמדות") });
        }
    }
    let mut names: Vec<&str> = sockets
        .iter()
        .map(|s| match *s {
            "type2" | "type2_cable" => "Type 2",
            "type2_combo" => "CCS",
            "chademo" => "CHAdeMO",
            "type1" => "Type 1",
            "type1_combo" => "CCS1",
            "tesla_supercharger" | "tesla_destination" => "Tesla",
            "schuko" | "typee" => "שקע ביתי",
            "gb_t" | "gb_dc" => "GB/T",
            _ => "",
        })
        .filter(|s| !s.is_empty())
        .collect();
    names.dedup();
    let mut uniq: Vec<&str> = Vec::new();
    for n in names {
        if !uniq.contains(&n) {
            uniq.push(n);
        }
    }
    if !uniq.is_empty() {
        parts.push(uniq.join(", "));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phones() {
        assert_eq!(phone_key("+972 3-612-3456").as_deref(), Some("036123456"));
        assert_eq!(phone_key("03-6123456").as_deref(), Some("036123456"));
        assert_eq!(phone_key("+972-52-1234567; 03-1").as_deref(), Some("0521234567"));
        assert_eq!(phone_key("*2345").as_deref(), None);
        assert_eq!(phone_key("1-700-700-700").as_deref(), Some("1700700700"));
        assert_eq!(phone_key("call us"), None);
    }

    #[test]
    fn json_roundtrip_and_charging() {
        let i = PoiInfo { phone: Some("03".into()), brand: Some("פז".into()), ..Default::default() };
        let j = i.to_json().unwrap();
        assert_eq!(PoiInfo::from_json(Some(&j)), Some(i));
        assert_eq!(PoiInfo::default().to_json(), None);
        assert_eq!(charging_line("4", &["type2", "type2_combo", "type2"]).as_deref(), Some("4 עמדות · Type 2, CCS"));
        assert_eq!(charging_line("", &[]), None);
    }
}

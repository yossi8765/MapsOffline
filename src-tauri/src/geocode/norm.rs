//! Text normalization, query parsing and fuzzy-matching helpers for the geocoder.
use unicode_normalization::UnicodeNormalization;

/// Words that denote the "kind" of a street and are optional in queries.
const TYPE_WORDS: &[&str] = &["רחוב", "רח", "שד", "שדרות", "שדרת", "דרכ", "st", "street", "rd", "road", "ave", "avenue"];

fn fold_letter(c: char) -> char {
    match c {
        'ך' => 'כ',
        'ם' => 'מ',
        'ן' => 'נ',
        'ף' => 'פ',
        'ץ' => 'צ',
        _ => c,
    }
}

/// Lower-case, strip niqqud / diacritics / geresh / gershayim, hyphens and punctuation -> spaces,
/// fold Hebrew final letters, collapse whitespace.
pub fn norm(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_space = true;
    for c in s.nfkd() {
        let cp = c as u32;
        if (0x0591..=0x05BD).contains(&cp)
            || cp == 0x05BF
            || (0x05C1..=0x05C2).contains(&cp)
            || (0x05C4..=0x05C5).contains(&cp)
            || cp == 0x05C7
            || (0x0300..=0x036F).contains(&cp)
            || (0x064B..=0x065F).contains(&cp)
            || cp == 0x0670
            || cp == 0x0640
        {
            continue;
        }
        if matches!(c, '\'' | '\u{2019}' | '\u{2018}' | '`' | '\u{00B4}' | '\u{05F3}' | '"' | '\u{05F4}' | '\u{201D}' | '\u{201C}') {
            continue;
        }
        if c.is_alphanumeric() {
            for l in c.to_lowercase() {
                out.push(fold_letter(l));
            }
            last_space = false;
        } else if !last_space {
            out.push(' ');
            last_space = true;
        }
    }
    while out.ends_with(' ') {
        out.pop();
    }
    out
}

pub fn tokens(s: &str) -> Vec<String> {
    norm(s).split(' ').filter(|t| !t.is_empty()).map(str::to_string).collect()
}

pub fn is_type_word(t: &str) -> bool {
    TYPE_WORDS.contains(&t)
}

/// Token with the leading definite article "ה" removed (only for tokens of >= 4 chars).
pub fn strip_he(t: &str) -> &str {
    if t.chars().count() >= 4 {
        if let Some(r) = t.strip_prefix('ה') {
            return r;
        }
    }
    t
}

/// Canonical comparison key: street-type words dropped, leading "ה" dropped from each token.
pub fn canon_tokens(toks: &[String]) -> Vec<String> {
    let kept: Vec<&String> = toks.iter().filter(|t| !is_type_word(t)).collect();
    let src: Vec<&String> = if kept.is_empty() { toks.iter().collect() } else { kept };
    src.into_iter().map(|t| strip_he(t).to_string()).collect()
}

pub fn canon(s: &str) -> String {
    canon_tokens(&tokens(s)).join(" ")
}

/// Spelling-variant key for a token: Hebrew is written with or without doubled yod / vav
/// ("מיימון" / "מימון", "קוולר" / "קולר"); runs of י and ו collapse to one letter. Index and query
/// are both folded, so either spelling finds the other.
pub fn spell_fold(t: &str) -> String {
    let mut out = String::with_capacity(t.len());
    let mut prev: Option<char> = None;
    for c in t.chars() {
        if (c == 'י' || c == 'ו') && prev == Some(c) {
            continue;
        }
        out.push(c);
        prev = Some(c);
    }
    out
}

/// Space separated bag of unique tokens to index in FTS for the given names:
/// every token, its form without a leading "ה", and the spelling-folded forms of both.
pub fn index_tokens(names: &[&str]) -> String {
    let mut seen: Vec<String> = Vec::new();
    for n in names {
        for t in tokens(n) {
            let s = strip_he(&t).to_string();
            for v in [spell_fold(&t), spell_fold(&s), t, s] {
                if !seen.contains(&v) {
                    seen.push(v);
                }
            }
        }
    }
    seen.join(" ")
}

/// (key, numeric prefix) for a house number string such as "12", "12א", "12/3".
pub fn housenumber_key(hn: &str) -> (String, Option<i64>) {
    let key: String = hn
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '/')
        .flat_map(|c| c.to_lowercase())
        .collect();
    let digits: String = hn.trim().chars().take_while(|c| c.is_ascii_digit()).collect();
    (key, digits.parse::<i64>().ok())
}

// ----------------------------------------------------------------- query rewriting

/// Common abbreviations of city names as they come out of `norm` (quotes and hyphens are
/// already stripped): "ת\"א" -> "תא", "י-ם" -> "י ם".
const CITY_ABBREV: &[(&str, &str)] = &[
    ("תא", "תל אביב"),
    ("בש", "באר שבע"),
    ("פת", "פתח תקווה"),
    ("רג", "רמת גן"),
    ("י מ", "ירושלים"),
    ("כס", "כפר סבא"),
    ("בב", "בני ברק"),
    ("ראשלצ", "ראשון לציון"),
    ("רשלצ", "ראשון לציון"),
    ("קש", "קריית שמונה"),
    ("קג", "קריית גת"),
    // Ramat HaSharon is written רמה"ש; "רחל" is a common street name (Rachel Imenu), never a city
    ("רמהש", "רמת השרון"),
];

/// Expand city abbreviations that stand as whole tokens ("הרצל 12 תא" -> "הרצל 12 תל אביב").
pub fn expand_abbreviations(q: &str) -> String {
    let mut s = format!(" {} ", norm(q));
    for (short, full) in CITY_ABBREV {
        let pat = format!(" {short} ");
        if s.contains(&pat) {
            s = s.replace(&pat, &format!(" {full} "));
        }
    }
    s.trim().to_string()
}

/// The Hebrew standard keyboard layout under a Latin QWERTY key: "sh,bdu;" was meant as
/// "דיזנגוף" with the keyboard left in English. Returns None when the text has no such letters.
pub fn from_latin_keyboard(q: &str) -> Option<String> {
    const MAP: &[(char, char)] = &[
        ('q', '/'), ('w', '\''), ('e', 'ק'), ('r', 'ר'), ('t', 'א'), ('y', 'ט'), ('u', 'ו'), ('i', 'ן'), ('o', 'ם'), ('p', 'פ'),
        ('a', 'ש'), ('s', 'ד'), ('d', 'ג'), ('f', 'כ'), ('g', 'ע'), ('h', 'י'), ('j', 'ח'), ('k', 'ל'), ('l', 'ך'), (';', 'ף'),
        ('z', 'ז'), ('x', 'ס'), ('c', 'ב'), ('v', 'ה'), ('b', 'נ'), ('n', 'מ'), ('m', 'צ'), (',', 'ת'), ('.', 'ץ'), ('\'', ','),
    ];
    let mut out = String::with_capacity(q.len() * 2);
    let mut mapped = 0;
    for c in q.chars() {
        let l = c.to_ascii_lowercase();
        match MAP.iter().find(|(k, _)| *k == l) {
            Some((_, h)) => {
                out.push(*h);
                mapped += 1;
            }
            None if c.is_ascii_alphabetic() => return None, // a real Latin word (e.g. "Caesarea")
            None => out.push(c),
        }
    }
    (mapped >= 2).then_some(out)
}

// ----------------------------------------------------------------- address parsing

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddrCand {
    /// Canonical street key.
    pub street: String,
    /// Canonical city key (None = any city).
    pub city: Option<String>,
    /// House number key ("12", "12א").
    pub hn: String,
    pub num: i64,
}

fn is_hn_token(t: &str) -> bool {
    let mut it = t.chars().peekable();
    let mut digits = 0;
    while let Some(c) = it.peek() {
        if c.is_ascii_digit() {
            digits += 1;
            it.next();
        } else {
            break;
        }
    }
    if digits == 0 || digits > 5 {
        return false;
    }
    let rest: Vec<char> = it.collect();
    rest.is_empty() || (rest.len() == 1 && rest[0].is_alphabetic())
}

fn mk(street: &[String], city: &[String], hn: &str) -> Option<AddrCand> {
    let s = canon_tokens(street).join(" ");
    if s.is_empty() || s.chars().all(|c| c.is_ascii_digit() || c == ' ') {
        return None;
    }
    let c = canon_tokens(city).join(" ");
    let (key, num) = housenumber_key(hn);
    Some(AddrCand { street: s, city: if c.is_empty() { None } else { Some(c) }, hn: key, num: num? })
}

/// All plausible (street, number, city) readings of the query, best first.
pub fn address_candidates(q: &str) -> Vec<AddrCand> {
    let toks = tokens(q);
    let mut out: Vec<AddrCand> = Vec::new();
    for p in (0..toks.len()).rev() {
        if !is_hn_token(&toks[p]) {
            continue;
        }
        let before = &toks[..p];
        let after = &toks[p + 1..];
        let hn = &toks[p];
        if !before.is_empty() && !after.is_empty() {
            out.extend(mk(before, after, hn));
        }
        if after.is_empty() {
            for k in 0..before.len() {
                out.extend(mk(&before[k..], &before[..k], hn));
            }
        } else if before.is_empty() {
            for k in 1..=after.len() {
                out.extend(mk(&after[..k], &after[k..], hn));
            }
        } else {
            // "street 12 city" handled above; also allow "city street 12 extra"? no.
            out.extend(mk(before, &[], hn));
        }
    }
    out.dedup();
    out
}

// ----------------------------------------------------------------- fuzzy

fn confusable(a: char, b: char) -> bool {
    const GROUPS: &[&[char]] = &[&['א', 'ע'], &['כ', 'ק'], &['ת', 'ט'], &['ס', 'ש'], &['ח', 'כ'], &['ב', 'ו']];
    GROUPS.iter().any(|g| g.contains(&a) && g.contains(&b))
}

/// Levenshtein distance scaled by 2: insert/delete = 2, substitute = 2, substitute of a commonly
/// confused letter pair = 1.
pub fn levenshtein2(a: &str, b: &str) -> u32 {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return 2 * b.len() as u32;
    }
    if b.is_empty() {
        return 2 * a.len() as u32;
    }
    let mut prev: Vec<u32> = (0..=b.len() as u32).map(|i| i * 2).collect();
    let mut cur = vec![0u32; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i as u32 * 2;
        for j in 1..=b.len() {
            let sub = if a[i - 1] == b[j - 1] {
                0
            } else if confusable(a[i - 1], b[j - 1]) {
                1
            } else {
                2
            };
            cur[j] = (prev[j - 1] + sub).min(prev[j] + 2).min(cur[j - 1] + 2);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Plain integer Levenshtein distance (unit costs).
pub fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let c = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j - 1] + c).min(prev[j] + 1).min(cur[j - 1] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Alternative spellings of the first `n` chars of a token (confusable letters substituted).
pub fn prefix_variants(tok: &str, n: usize) -> Vec<String> {
    const ALT: &[(char, char)] = &[('א', 'ע'), ('ע', 'א'), ('כ', 'ק'), ('ק', 'כ'), ('ת', 'ט'), ('ט', 'ת'), ('ס', 'ש'), ('ש', 'ס')];
    let chars: Vec<char> = tok.chars().take(n).collect();
    let mut out: Vec<Vec<char>> = vec![vec![]];
    for c in chars {
        let mut next = Vec::new();
        for base in &out {
            let mut a = base.clone();
            a.push(c);
            next.push(a);
            if let Some((_, alt)) = ALT.iter().find(|(x, _)| *x == c) {
                let mut b = base.clone();
                b.push(*alt);
                next.push(b);
            }
        }
        out = next;
    }
    out.into_iter().map(|v| v.into_iter().collect()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization() {
        assert_eq!(norm("תל־אביב-יפו"), "תל אביב יפו");
        assert_eq!(norm("רחוב הרצל"), "רחוב הרצל");
        assert_eq!(norm("בֵּית שְׁאָן"), "בית שאנ");
        assert_eq!(norm("ג'ורג' וושינגטון"), "גורג וושינגטונ");
        assert_eq!(norm("רמב\"ם"), "רמבמ");
        assert_eq!(norm("  Café  Roma "), "cafe roma");
        assert_eq!(norm("מלך"), "מלכ");
    }

    #[test]
    fn city_abbreviations() {
        assert_eq!(expand_abbreviations("הרצל 12 ת\"א"), "הרצל 12 תל אביב");
        assert_eq!(expand_abbreviations("סוקולוב 5 רמה\"ש"), "סוקולוב 5 רמת השרון");
        // "רחל" is a street (Rachel Imenu), not Ramat HaSharon
        assert_eq!(expand_abbreviations("רחל 12 ירושלים"), "רחל 12 ירושלימ");
    }

    #[test]
    fn canonical() {
        assert_eq!(canon("רחוב הרצל"), "רצל");
        assert_eq!(canon("רח' הרצל"), "רצל");
        assert_eq!(canon("הרצל"), "רצל");
        assert_eq!(canon("שדרות רוטשילד"), "רוטשילד");
        assert_eq!(canon("שד' רוטשילד"), "רוטשילד");
        assert_eq!(canon("דרך מנחם בגין"), "מנחמ בגינ");
        assert_eq!(canon("דרך"), "דרכ");
        assert_eq!(canon("בן יהודה"), "בנ יהודה");
        assert_eq!(index_tokens(&["הרצל"]), "הרצל רצל");
        assert_eq!(spell_fold("מיימונ"), "מימונ");
        assert_eq!(spell_fold("קוולר"), "קולר");
        assert!(index_tokens(&["מיימון"]).split(' ').any(|t| t == "מימונ"));
    }

    #[test]
    fn housenumbers() {
        assert_eq!(housenumber_key("12א"), ("12א".to_string(), Some(12)));
        assert_eq!(housenumber_key("12 A"), ("12a".to_string(), Some(12)));
        assert_eq!(housenumber_key("x"), ("x".to_string(), None));
    }

    fn cand(street: &str, city: Option<&str>, hn: &str, num: i64) -> AddrCand {
        AddrCand { street: street.into(), city: city.map(Into::into), hn: hn.into(), num }
    }

    #[test]
    fn address_parsing() {
        let c = address_candidates("הרצל 12, תל אביב");
        assert_eq!(c[0], cand("רצל", Some("תל אביב"), "12", 12));
        let c = address_candidates("הרצל 12 תל אביב");
        assert_eq!(c[0], cand("רצל", Some("תל אביב"), "12", 12));
        let c = address_candidates("הרצל 12");
        assert_eq!(c[0], cand("רצל", None, "12", 12));
        let c = address_candidates("תל אביב הרצל 12");
        assert!(c.contains(&cand("רצל", Some("תל אביב"), "12", 12)));
        assert!(c.contains(&cand("אביב רצל", Some("תל"), "12", 12)));
        let c = address_candidates("תל אביב, רחוב הרצל 12א");
        assert!(c.contains(&cand("רצל", Some("תל אביב"), "12א", 12)));
        let c = address_candidates("12 הרצל תל אביב");
        assert_eq!(c[0], cand("רצל", Some("תל אביב"), "12", 12));
        assert!(address_candidates("הרצל").is_empty());
        assert!(address_candidates("12").is_empty());
    }

    #[test]
    fn query_rewrites() {
        assert_eq!(expand_abbreviations("הרצל 12 ת\"א"), "הרצל 12 תל אביב");
        assert_eq!(expand_abbreviations("ז'בוטינסקי 5 פ״ת"), "זבוטינסקי 5 פתח תקווה");
        assert_eq!(expand_abbreviations("יפו 7 י-ם"), "יפו 7 ירושלים");
        assert_eq!(expand_abbreviations("תאטרון"), "תאטרונ");
        assert_eq!(from_latin_keyboard("sh,bdu;").as_deref(), Some("דיתנגוף"));
        assert_eq!(from_latin_keyboard("shzbdu; 50").as_deref(), Some("דיזנגוף 50"));
        assert!(from_latin_keyboard("caesarea").is_some(), "Latin words are converted too; search() keeps whichever reading scores better");
        assert_eq!(from_latin_keyboard("הרצל"), None);
    }

    #[test]
    fn fuzzy() {
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein2("אזריאלי", "עזריאלי"), 1);
        assert!(levenshtein2("הרצל", "הרצלל") == 2);
        let v = prefix_variants("אזר", 3);
        assert!(v.contains(&"אזר".to_string()) && v.contains(&"עזר".to_string()));
    }
}

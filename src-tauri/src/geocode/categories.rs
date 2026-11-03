//! Hebrew POI category lexicon: query phrases such as "בית כנסת" / "תחנת דלק" are mapped to the
//! OSM category values stored in `pois.category`, and category values get their Hebrew label.
//!
//! Category values are the raw OSM tag values (`supermarket`, `place_of_worship`, …). Places of
//! worship carry the religion as a suffix (`place_of_worship:jewish`) so that synagogues, mosques
//! and churches can be told apart both in the label and in the lexicon.
use std::sync::OnceLock;

use super::norm::{strip_he, tokens};

/// (query phrase, category values it stands for). A value without `:` matches every category
/// with that base; a value with `:` matches exactly.
const LEXICON: &[(&str, &[&str])] = &[
    // religion
    ("בית כנסת", &["place_of_worship:jewish"]),
    ("בית הכנסת", &["place_of_worship:jewish"]),
    ("בתי כנסת", &["place_of_worship:jewish"]),
    ("ביהכנס", &["place_of_worship:jewish"]),
    ("ביהכנ", &["place_of_worship:jewish"]),
    ("בית תפילה", &["place_of_worship"]),
    ("ישיבה", &["place_of_worship:jewish", "college", "school"]),
    ("ישיבת", &["place_of_worship:jewish", "college", "school"]),
    ("כולל", &["place_of_worship:jewish"]),
    ("בית מדרש", &["place_of_worship:jewish"]),
    ("בית המדרש", &["place_of_worship:jewish"]),
    ("ביהמד", &["place_of_worship:jewish"]),
    ("מסגד", &["place_of_worship:muslim"]),
    ("כנסייה", &["place_of_worship:christian"]),
    ("כנסיה", &["place_of_worship:christian"]),
    ("כנסיית", &["place_of_worship:christian"]),
    ("מקווה", &["mikveh", "mikvah", "public_bath"]),
    ("מקוה", &["mikveh", "mikvah", "public_bath"]),
    ("בית עלמין", &["grave_yard", "cemetery"]),
    ("בית קברות", &["grave_yard", "cemetery"]),
    // education
    ("בית ספר", &["school"]),
    ("בתי ספר", &["school"]),
    ("ביהס", &["school"]),
    ("ביס", &["school"]),
    ("תיכון", &["school"]),
    ("גן ילדים", &["kindergarten"]),
    ("גני ילדים", &["kindergarten"]),
    ("גנון", &["kindergarten"]),
    ("מעון", &["kindergarten", "childcare"]),
    ("פעוטון", &["kindergarten", "childcare"]),
    ("אוניברסיטה", &["university"]),
    ("אוניברסיטת", &["university"]),
    ("מכללה", &["college"]),
    ("מכללת", &["college"]),
    ("ספרייה", &["library"]),
    ("ספריה", &["library"]),
    ("ספריית", &["library"]),
    // health
    ("בית חולים", &["hospital"]),
    ("בית החולים", &["hospital"]),
    ("בתי חולים", &["hospital"]),
    ("ביח", &["hospital"]),
    ("מרכז רפואי", &["hospital", "clinic"]),
    ("קופת חולים", &["clinic", "doctors"]),
    ("קופח", &["clinic", "doctors"]),
    ("מרפאה", &["clinic", "doctors"]),
    ("מרפאת", &["clinic", "doctors", "dentist"]),
    ("רופא", &["doctors", "clinic"]),
    ("רופאים", &["doctors", "clinic"]),
    ("רופא שיניים", &["dentist"]),
    ("מרפאת שיניים", &["dentist"]),
    ("שיניים", &["dentist"]),
    ("וטרינר", &["veterinary"]),
    ("וטרינרי", &["veterinary"]),
    ("בית מרקחת", &["pharmacy"]),
    ("מרקחת", &["pharmacy"]),
    ("פארם", &["pharmacy", "chemist"]),
    ("סופר פארם", &["pharmacy", "chemist"]),
    ("בית אבות", &["nursing_home", "social_facility"]),
    ("דיור מוגן", &["nursing_home", "social_facility"]),
    // food & shopping
    ("סופר", &["supermarket", "convenience"]),
    ("סופרמרקט", &["supermarket"]),
    ("סופר מרקט", &["supermarket"]),
    ("מכולת", &["convenience", "supermarket"]),
    ("מינימרקט", &["convenience", "supermarket"]),
    ("מיני מרקט", &["convenience", "supermarket"]),
    ("מסעדה", &["restaurant", "fast_food"]),
    ("מסעדת", &["restaurant", "fast_food"]),
    ("מסעדות", &["restaurant", "fast_food"]),
    ("בית קפה", &["cafe"]),
    ("קפה", &["cafe"]),
    ("קפיטריה", &["cafe"]),
    ("מזון מהיר", &["fast_food"]),
    ("פלאפל", &["fast_food", "restaurant"]),
    ("פיצה", &["fast_food", "restaurant"]),
    ("פיצריה", &["fast_food", "restaurant"]),
    ("שווארמה", &["fast_food", "restaurant"]),
    ("שוארמה", &["fast_food", "restaurant"]),
    ("המבורגר", &["fast_food", "restaurant"]),
    ("מאפייה", &["bakery"]),
    ("מאפיה", &["bakery"]),
    ("קונדיטוריה", &["bakery", "pastry"]),
    ("קניון", &["mall"]),
    ("מרכז קניות", &["mall"]),
    ("מרכז מסחרי", &["mall"]),
    ("שוק", &["marketplace"]),
    ("חנות בגדים", &["clothes"]),
    ("בגדים", &["clothes"]),
    ("חנות ספרים", &["books"]),
    ("חנות נעליים", &["shoes"]),
    ("חנות חשמל", &["electronics"]),
    ("אלקטרוניקה", &["electronics"]),
    ("חנות צעצועים", &["toys"]),
    ("חנות פרחים", &["florist"]),
    ("מספרה", &["hairdresser"]),
    ("מכבסה", &["laundry", "dry_cleaning"]),
    // money & services
    ("בנק", &["bank"]),
    ("סניף בנק", &["bank"]),
    ("כספומט", &["atm"]),
    ("בנקט", &["atm"]),
    ("דואר", &["post_office"]),
    ("סניף דואר", &["post_office"]),
    ("בית דואר", &["post_office"]),
    ("משטרה", &["police"]),
    ("תחנת משטרה", &["police"]),
    ("כיבוי אש", &["fire_station"]),
    ("תחנת כיבוי", &["fire_station"]),
    ("עירייה", &["townhall", "government"]),
    ("עיריית", &["townhall", "government"]),
    ("מועצה", &["townhall", "government"]),
    ("מועצה מקומית", &["townhall", "government"]),
    ("מועצה אזורית", &["townhall", "government"]),
    ("מתנס", &["community_centre"]),
    ("מרכז קהילתי", &["community_centre"]),
    ("בית העם", &["community_centre"]),
    // transport
    ("תחנת דלק", &["fuel"]),
    ("תחנת תדלוק", &["fuel"]),
    ("דלק", &["fuel"]),
    ("חניון", &["parking"]),
    ("חניה", &["parking"]),
    ("חנייה", &["parking"]),
    ("תחנת רכבת", &["station", "halt"]),
    ("רכבת", &["station", "halt"]),
    ("תחנה מרכזית", &["bus_station"]),
    ("תחנת אוטובוס מרכזית", &["bus_station"]),
    ("שדה תעופה", &["aerodrome", "terminal"]),
    ("נמל תעופה", &["aerodrome", "terminal"]),
    ("מוסך", &["car_repair"]),
    ("שטיפת רכב", &["car_wash"]),
    ("שטיפת מכוניות", &["car_wash"]),
    ("השכרת רכב", &["car_rental"]),
    // lodging
    ("מלון", &["hotel"]),
    ("בית מלון", &["hotel"]),
    ("מלונות", &["hotel"]),
    ("אכסניה", &["hostel", "guest_house"]),
    ("אכסניית", &["hostel", "guest_house"]),
    ("הוסטל", &["hostel"]),
    ("צימר", &["guest_house", "chalet", "apartment"]),
    ("צימרים", &["guest_house", "chalet", "apartment"]),
    // leisure & culture
    ("קולנוע", &["cinema"]),
    ("בית קולנוע", &["cinema"]),
    ("סינמה", &["cinema"]),
    ("תיאטרון", &["theatre"]),
    ("תאטרון", &["theatre"]),
    ("מוזיאון", &["museum"]),
    ("מוזאון", &["museum"]),
    ("חדר כושר", &["fitness_centre", "gym", "sports_centre"]),
    ("מכון כושר", &["fitness_centre", "gym", "sports_centre"]),
    ("כושר", &["fitness_centre", "gym", "sports_centre"]),
    ("בריכה", &["swimming_pool", "water_park"]),
    ("בריכת שחייה", &["swimming_pool"]),
    ("בריכת שחיה", &["swimming_pool"]),
    ("קאנטרי", &["sports_centre", "swimming_pool"]),
    ("מרכז ספורט", &["sports_centre"]),
    ("אצטדיון", &["stadium"]),
    ("מגרש", &["pitch", "sports_centre"]),
    ("מגרש כדורגל", &["pitch", "stadium"]),
    ("מגרש ספורט", &["pitch", "sports_centre"]),
    ("גן שעשועים", &["playground"]),
    ("גינת משחקים", &["playground"]),
    ("מגרש משחקים", &["playground"]),
    ("פארק", &["park", "garden"]),
    ("גינה", &["garden", "park"]),
    ("גינת", &["garden", "park"]),
    ("גן ציבורי", &["park", "garden"]),
    ("פארק ציבורי", &["park"]),
    ("חוף", &["beach"]),
    ("חוף ים", &["beach"]),
    ("חוף רחצה", &["beach"]),
    ("מעיין", &["spring"]),
    ("תצפית", &["viewpoint"]),
    ("נקודת תצפית", &["viewpoint"]),
    ("מצפה", &["viewpoint", "observatory"]),
    ("אתר ארכאולוגי", &["archaeological_site", "ruins"]),
    ("עתיקות", &["archaeological_site", "ruins"]),
    ("אנדרטה", &["memorial", "monument"]),
    ("אנדרטת", &["memorial", "monument"]),
    ("יד לבנים", &["memorial", "monument"]),
    ("אטרקציה", &["attraction"]),
];

/// Hebrew label of a stored category value.
pub fn label(cat: &str) -> String {
    let (base, sub) = match cat.split_once(':') {
        Some((b, s)) => (b, s),
        None => (cat, ""),
    };
    match base {
        "place_of_worship" => match sub {
            "jewish" => "בית כנסת",
            "muslim" => "מסגד",
            "christian" => "כנסייה",
            _ => "בית תפילה",
        },
        "mall" => "קניון",
        "restaurant" => "מסעדה",
        "cafe" => "בית קפה",
        "fast_food" => "מזון מהיר",
        "bank" => "בנק",
        "atm" => "כספומט",
        "pharmacy" | "chemist" => "בית מרקחת",
        "hospital" => "בית חולים",
        "clinic" | "doctors" => "מרפאה",
        "dentist" => "רופא שיניים",
        "veterinary" => "וטרינר",
        "nursing_home" | "social_facility" => "בית אבות",
        "school" => "בית ספר",
        "university" => "אוניברסיטה",
        "college" => "מכללה",
        "kindergarten" | "childcare" => "גן ילדים",
        "library" => "ספרייה",
        "fuel" => "תחנת דלק",
        "police" => "משטרה",
        "fire_station" => "כיבוי אש",
        "post_office" => "דואר",
        "theatre" => "תיאטרון",
        "cinema" => "קולנוע",
        "supermarket" => "סופרמרקט",
        "convenience" => "מכולת",
        "bakery" | "pastry" => "מאפייה",
        "hotel" => "מלון",
        "hostel" => "הוסטל",
        "guest_house" | "chalet" => "צימר",
        "apartment" => "דירת נופש",
        "museum" => "מוזיאון",
        "attraction" => "אטרקציה",
        "viewpoint" => "תצפית",
        "park" => "פארק",
        "garden" => "גן",
        "playground" => "גן שעשועים",
        "stadium" => "אצטדיון",
        "pitch" => "מגרש ספורט",
        "sports_centre" => "מרכז ספורט",
        "fitness_centre" | "gym" => "חדר כושר",
        "swimming_pool" => "בריכה",
        "water_park" => "פארק מים",
        "station" | "halt" => "תחנת רכבת",
        "bus_station" => "תחנה מרכזית",
        "aerodrome" => "שדה תעופה",
        "terminal" => "טרמינל",
        "parking" => "חניון",
        "car_repair" => "מוסך",
        "car_wash" => "שטיפת רכב",
        "car_rental" => "השכרת רכב",
        "bicycle_rental" => "השכרת אופניים",
        "charging_station" => "עמדת טעינה",
        "taxi" => "תחנת מוניות",
        "beach" => "חוף",
        "peak" => "פסגה",
        "spring" => "מעיין",
        "monument" => "אנדרטה",
        "memorial" => "אנדרטת זיכרון",
        "archaeological_site" => "אתר ארכאולוגי",
        "ruins" => "חורבה",
        "townhall" | "government" => "מוסד ממשלתי",
        "community_centre" => "מתנ״ס",
        "marketplace" => "שוק",
        "clothes" => "חנות בגדים",
        "shoes" => "חנות נעליים",
        "books" => "חנות ספרים",
        "toys" => "חנות צעצועים",
        "florist" => "חנות פרחים",
        "electronics" => "חנות חשמל",
        "hairdresser" => "מספרה",
        "laundry" | "dry_cleaning" => "מכבסה",
        "department_store" => "חנות כלבו",
        "mikveh" | "mikvah" | "public_bath" => "מקווה",
        "grave_yard" | "cemetery" => "בית עלמין",
        "public_building" => "מבנה ציבור",
        "company" | "office" => "משרד",
        "information" => "מרכז מידע",
        "artwork" => "יצירת אמנות",
        "observatory" => "מצפה",
        other => other,
    }
    .to_string()
}

/// Does the stored category `cat` belong to the category set `set`?
pub fn matches(cat: &str, set: &[&str]) -> bool {
    let base = cat.split_once(':').map_or(cat, |(b, _)| b);
    set.iter().any(|s| if s.contains(':') { *s == cat } else { *s == base })
}

struct Entry {
    toks: Vec<String>,
    cats: &'static [&'static str],
}

fn entries() -> &'static [Entry] {
    static E: OnceLock<Vec<Entry>> = OnceLock::new();
    E.get_or_init(|| {
        let mut v: Vec<Entry> = LEXICON
            .iter()
            .map(|(p, cats)| Entry { toks: tokens(p).iter().map(|t| strip_he(t).to_string()).collect(), cats })
            .filter(|e| !e.toks.is_empty())
            .collect();
        // longest phrases first so the scan below prefers them
        v.sort_by_key(|c| std::cmp::Reverse(c.toks.len()));
        v
    })
}

/// A category phrase found in a query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatMatch {
    /// Index range of the matched tokens in the query token list.
    pub start: usize,
    pub end: usize,
    pub cats: &'static [&'static str],
}

/// Find the longest category phrase in normalized query tokens (`norm::tokens`). The last query
/// token may be an unfinished prefix (>= 2 chars) of the phrase's last word, so the category is
/// recognised while the user is still typing ("בית כנ").
pub fn detect(q: &[String]) -> Option<CatMatch> {
    if q.is_empty() {
        return None;
    }
    let qs: Vec<&str> = q.iter().map(|t| strip_he(t)).collect();
    let mut best: Option<CatMatch> = None;
    for e in entries() {
        let n = e.toks.len();
        if let Some(b) = &best {
            if b.end - b.start > n {
                break;
            }
        }
        if n > qs.len() {
            continue;
        }
        for start in 0..=qs.len() - n {
            let end = start + n;
            let full = (0..n).all(|i| qs[start + i] == e.toks[i]);
            let typing = !full
                && end == qs.len()
                && n >= 2
                && (0..n - 1).all(|i| qs[start + i] == e.toks[i])
                && qs[end - 1].chars().count() >= 2
                && e.toks[n - 1].starts_with(qs[end - 1]);
            if full || typing {
                let better = match &best {
                    None => true,
                    Some(b) => n > b.end - b.start || (n == b.end - b.start && start < b.start),
                };
                if better {
                    best = Some(CatMatch { start, end, cats: e.cats });
                }
                break;
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> Vec<String> {
        tokens(s)
    }

    #[test]
    fn detects_phrases() {
        let m = detect(&t("בית כנסת חניכי הישיבות")).unwrap();
        assert_eq!((m.start, m.end), (0, 2));
        assert!(m.cats.contains(&"place_of_worship:jewish"));
        let m = detect(&t("בית הכנסת הגדול")).unwrap();
        assert_eq!((m.start, m.end), (0, 2));
        let m = detect(&t("ביה\"ס אורט")).unwrap();
        assert_eq!((m.start, m.end), (0, 1));
        assert_eq!(m.cats, &["school"]);
        let m = detect(&t("תחנת דלק")).unwrap();
        assert_eq!(m.cats, &["fuel"]);
        // longest match wins over the shorter "דלק"
        assert_eq!((m.start, m.end), (0, 2));
        let m = detect(&t("סופר פארם רוטשילד")).unwrap();
        assert_eq!((m.start, m.end), (0, 2));
        assert!(m.cats.contains(&"pharmacy"));
        assert!(detect(&t("הרצל 12")).is_none());
        assert!(detect(&t("תל אביב")).is_none());
    }

    #[test]
    fn detects_while_typing() {
        let m = detect(&t("בית כנ")).unwrap();
        assert_eq!((m.start, m.end), (0, 2));
        assert!(detect(&t("בית כ")).is_none());
        assert!(detect(&t("בית")).is_none());
    }

    #[test]
    fn labels_and_matching() {
        assert_eq!(label("place_of_worship:jewish"), "בית כנסת");
        assert_eq!(label("place_of_worship:muslim"), "מסגד");
        assert_eq!(label("place_of_worship"), "בית תפילה");
        assert_eq!(label("supermarket"), "סופרמרקט");
        assert!(matches("place_of_worship:jewish", &["place_of_worship"]));
        assert!(matches("place_of_worship:jewish", &["place_of_worship:jewish"]));
        assert!(!matches("place_of_worship:muslim", &["place_of_worship:jewish"]));
        assert!(!matches("school", &["place_of_worship"]));
    }
}

//! Hidden niqqud for the voice: the instruction texts on screen stay unvocalised, but what is
//! sent to the text-to-speech engine gets full niqqud on the fixed words of the instruction
//! templates (routing/instructions.rs, nav/machine.rs, online/osrm.rs), so the Hebrew voice reads
//! "פְּנֵה" and not "פָּנָה", "הִגַּעְתָּ" and not "הגעת" as a noun. Street and place names are
//! not touched: they come from OpenStreetMap and only a full vocaliser could do them.

/// Multi-word phrases first: they settle words whose vowels depend on the context
/// ("צא לדרך" = לַדֶּרֶךְ, but "לדרך נמיר" = לְדֶרֶךְ).
const PHRASES: &[(&str, &str)] = &[
    ("צא לדרך", "צֵא לַדֶּרֶךְ"),
    ("היכנס לכיכר", "הִכָּנֵס לַכִּכָּר"),
    ("צא מהכיכר", "צֵא מֵהַכִּכָּר"),
    ("בצע פניית פרסה", "בַּצֵּעַ פְּנִיַּת פַּרְסָה"),
    ("הגעת ליעד", "הִגַּעְתָּ לַיַּעַד"),
    ("יצאת מהמסלול", "יָצָאתָ מֵהַמַּסְלוּל"),
    ("מחשב מסלול מחדש", "מְחַשֵּׁב מַסְלוּל מֵחָדָשׁ"),
    ("מצלמת מהירות", "מַצְלֵמַת מְהִירוּת"),
    ("קילומטר וחצי", "קִילוֹמֶטֶר וָחֵצִי"),
    ("המשך ישר", "הַמְשֵׁךְ יָשָׁר"),
];

/// Single words of the templates (whole tokens only).
const WORDS: &[(&str, &str)] = &[
    // verbs (masculine singular imperative, as on screen)
    ("פנה", "פְּנֵה"),
    ("המשך", "הַמְשֵׁךְ"),
    ("צא", "צֵא"),
    ("וצא", "וְצֵא"),
    ("היכנס", "הִכָּנֵס"),
    ("השתלב", "הִשְׁתַּלֵּב"),
    ("בצע", "בַּצֵּעַ"),
    ("הגעת", "הִגַּעְתָּ"),
    ("יצאת", "יָצָאתָ"),
    ("מחשב", "מְחַשֵּׁב"),
    // directions
    ("ימינה", "יָמִינָה"),
    ("שמאלה", "שְׂמֹאלָה"),
    ("קלות", "קַלּוֹת"),
    ("בחדות", "בַּחֲדוּת"),
    ("ישר", "יָשָׁר"),
    ("לכיוון", "לְכִוּוּן"),
    ("בהתפצלות", "בַּהִתְפַּצְּלוּת"),
    ("פניית", "פְּנִיַּת"),
    ("פרסה", "פַּרְסָה"),
    // roundabouts and exits
    ("ביציאה", "בַּיְּצִיאָה"),
    ("הראשונה", "הָרִאשׁוֹנָה"),
    ("השנייה", "הַשְּׁנִיָּה"),
    ("השלישית", "הַשְּׁלִישִׁית"),
    ("הרביעית", "הָרְבִיעִית"),
    ("החמישית", "הַחֲמִישִׁית"),
    ("השישית", "הַשִּׁשִּׁית"),
    ("מהכיכר", "מֵהַכִּכָּר"),
    // road kinds before a name (construct state)
    ("לרחוב", "לִרְחוֹב"),
    ("ברחוב", "בִּרְחוֹב"),
    ("רחוב", "רְחוֹב"),
    ("לכביש", "לִכְבִישׁ"),
    ("בכביש", "בִּכְבִישׁ"),
    ("כביש", "כְּבִישׁ"),
    ("לשדרות", "לִשְׂדֵרוֹת"),
    ("בשדרות", "בִּשְׂדֵרוֹת"),
    ("שדרות", "שְׂדֵרוֹת"),
    ("לדרך", "לְדֶרֶךְ"),
    ("בדרך", "בְּדֶרֶךְ"),
    ("דרך", "דֶּרֶךְ"),
    ("לכיכר", "לְכִכַּר"),
    ("בכיכר", "בְּכִכַּר"),
    ("לסמטת", "לְסִמְטַת"),
    ("בסמטת", "בְּסִמְטַת"),
    ("למחלף", "לְמֶחְלַף"),
    ("במחלף", "בְּמֶחְלַף"),
    // distances and timing
    ("בעוד", "בְּעוֹד"),
    ("מטר", "מֶטֶר"),
    ("מטרים", "מֶטְרִים"),
    ("קילומטר", "קִילוֹמֶטֶר"),
    ("וחצי", "וָחֵצִי"),
    ("ואז", "וְאָז"),
    ("אחרי", "אַחֲרֵי"),
    // the rest of the fixed sentences
    ("ליעד", "לַיַּעַד"),
    ("מהמסלול", "מֵהַמַּסְלוּל"),
    ("מסלול", "מַסְלוּל"),
    ("מחדש", "מֵחָדָשׁ"),
    ("מצלמת", "מַצְלֵמַת"),
    ("מהירות", "מְהִירוּת"),
];

use super::speech_names::{NAME_PHRASES, NAME_WORDS};

/// Number of vocalised entries (phrases + words, templates and names), for the report / tests.
pub const TABLE_SIZE: usize = PHRASES.len() + WORDS.len() + NAME_PHRASES.len() + NAME_WORDS.len();

const SHVA: char = '\u{05B0}';
const DAGESH: char = '\u{05BC}';

fn is_mark(c: char) -> bool {
    ('\u{0591}'..='\u{05C7}').contains(&c) && !matches!(c, '\u{05BE}' | '\u{05C0}' | '\u{05C3}' | '\u{05C6}')
}

/// `prefix` (ו / ל / ב / כ) glued to a vocalised word, with the vowel the grammar gives it:
/// before a shva the prefix takes a hiriq (לִרְחוֹב; before יְ the shva goes: לִירוּשָׁלַיִם, and
/// ו becomes וּ), before a hataf the matching short vowel, otherwise a shva (ו before ב/מ/פ: וּ);
/// a בגדכפ״ת letter after the prefix loses its dagesh kal.
pub fn with_prefix(prefix: char, word: &str) -> Option<String> {
    let mut chars = word.chars();
    let first = chars.next()?;
    let mut marks: Vec<char> = Vec::new();
    let mut rest = String::new();
    let mut in_marks = true;
    for c in chars {
        if in_marks && is_mark(c) {
            marks.push(c);
        } else {
            in_marks = false;
            rest.push(c);
        }
    }
    let vowel = marks.iter().copied().find(|c| matches!(*c, '\u{05B0}'..='\u{05BB}' | '\u{05C7}'));
    if "בגדכפת".contains(first) {
        marks.retain(|&c| c != DAGESH);
    }
    let mut out = String::new();
    out.push(prefix);
    match vowel {
        Some(SHVA) if first == 'י' => {
            // לִירוּשָׁלַיִם / וִירוּשָׁלַיִם: the prefix takes the hiriq, the yod loses its shva
            out.push('\u{05B4}');
            marks.retain(|&c| c != SHVA);
        }
        Some(SHVA) => out.push(if prefix == 'ו' { DAGESH } else { '\u{05B4}' }),
        Some('\u{05B1}') => out.push('\u{05B6}'), // hataf segol -> segol
        Some('\u{05B2}') => out.push('\u{05B7}'), // hataf patah -> patah
        Some('\u{05B3}') => out.push('\u{05B8}'), // hataf qamats -> qamats
        _ => out.push(if prefix == 'ו' && "במפ".contains(first) { DAGESH } else { SHVA }),
    }
    out.push(first);
    out.extend(marks);
    out.push_str(&rest);
    Some(out)
}

/// A name word with niqqud: itself, or after one of the prefixes ו / ל / ב / כ (not for words
/// starting with ה after ל / ב / כ: that may be the article, which the prefix would swallow).
fn name_word(token: &str) -> Option<String> {
    if let Some((_, v)) = NAME_WORDS.iter().find(|(w, _)| *w == token) {
        return Some(v.to_string());
    }
    let mut it = token.chars();
    let p = it.next()?;
    let base = it.as_str();
    if !"ולבכ".contains(p) || base.chars().count() < 2 {
        return None;
    }
    let (_, v) = NAME_WORDS.iter().find(|(w, _)| *w == base)?;
    if base.starts_with('ה') && p != 'ו' {
        return None;
    }
    with_prefix(p, v)
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '\'' || c == '׳' || c == '"' || c == '״' || ('\u{0591}'..='\u{05C7}').contains(&c)
}

/// Replace `from` by `to` where it stands as whole words (not inside a longer word).
fn replace_whole(text: &str, from: &str, to: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    let mut rest = text;
    while let Some(i) = rest.find(from) {
        let prev = rest[..i].chars().next_back().or_else(|| out.chars().next_back());
        let before_ok = prev.is_none_or(|c| !is_word_char(c));
        let after = &rest[i + from.len()..];
        let after_ok = after.chars().next().is_none_or(|c| !is_word_char(c));
        out.push_str(&rest[..i]);
        if before_ok && after_ok {
            out.push_str(to);
        } else {
            out.push_str(from);
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// The text to send to the voice: fixed template words with niqqud, names unchanged.
pub fn voice_text(display: &str) -> String {
    let mut s = display.to_string();
    for (from, to) in PHRASES {
        s = replace_whole(&s, from, to);
    }
    // multi-word names, after ו / ל / ב / כ and bare
    for (from, to) in NAME_PHRASES {
        for p in ['ו', 'ל', 'ב', 'כ'] {
            if let Some(pv) = with_prefix(p, to) {
                s = replace_whole(&s, &format!("{p}{from}"), &pv);
            }
        }
        s = replace_whole(&s, from, to);
    }
    // word by word: a token is replaced only when it is exactly a table word (a vocalised phrase
    // word is no longer equal to the plain one, so it is not touched twice)
    let mut out = String::with_capacity(s.len() + 16);
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        if !word.is_empty() {
            match WORDS.iter().find(|(w, _)| *w == word.as_str()) {
                Some((_, v)) => out.push_str(v),
                None => match name_word(word) {
                    Some(v) => out.push_str(&v),
                    None => out.push_str(word),
                },
            }
            word.clear();
        }
    };
    for c in s.chars() {
        if is_word_char(c) {
            word.push(c);
        } else {
            flush(&mut word, &mut out);
            out.push(c);
        }
    }
    flush(&mut word, &mut out);
    out
}

/// Sample sentences for the "test the voice" buttons in Settings.
pub const SAMPLES: [&str; 4] = ["בעוד 300 מטר פנה ימינה לרחוב ז׳בוטינסקי", "היכנס לכיכר וצא ביציאה השנייה לשדרות בן גוריון", "המשך ישר לכיוון ירושלים", "מצלמת מהירות בעוד 400 מטר"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_get_niqqud() {
        assert_eq!(voice_text("בעוד 300 מטר פנה ימינה לרחוב הרצל"), "בְּעוֹד 300 מֶטֶר פְּנֵה יָמִינָה לִרְחוֹב הֶרְצְל");
        assert_eq!(voice_text("היכנס לכיכר וצא ביציאה השנייה לכביש 1"), "הִכָּנֵס לַכִּכָּר וְצֵא בַּיְּצִיאָה הַשְּׁנִיָּה לִכְבִישׁ 1");
        assert_eq!(voice_text("הגעת ליעד"), "הִגַּעְתָּ לַיַּעַד");
        assert_eq!(voice_text("צא לדרך"), "צֵא לַדֶּרֶךְ");
        assert_eq!(voice_text("פנה שמאלה לדרך נמיר"), "פְּנֵה שְׂמֹאלָה לְדֶרֶךְ נמיר");
        assert_eq!(voice_text("המשך 2 קילומטר ואז פנה קלות ימינה"), "הַמְשֵׁךְ 2 קִילוֹמֶטֶר וְאָז פְּנֵה קַלּוֹת יָמִינָה");
        assert_eq!(voice_text("מצלמת מהירות בעוד קילומטר וחצי"), "מַצְלֵמַת מְהִירוּת בְּעוֹד קִילוֹמֶטֶר וָחֵצִי");
    }

    #[test]
    fn names_are_untouched() {
        // street names that merely contain a table word inside them stay as they are
        assert_eq!(voice_text("פנה ימינה לרחוב הרב בצלאל ז'ולטי"), "פְּנֵה יָמִינָה לִרְחוֹב הָרַב בצלאל ז'ולטי");
        assert_eq!(voice_text("צא ביציאה 5 לכיוון ירושלים, תל אביב"), "צֵא בַּיְּצִיאָה 5 לְכִוּוּן יְרוּשָׁלַיִם, תֵּל אָבִיב");
        assert_eq!(voice_text("פנה ימינה לרחוב ישראלי"), "פְּנֵה יָמִינָה לִרְחוֹב ישראלי");
        assert_eq!(voice_text("Main Street"), "Main Street");
    }

    #[test]
    fn display_text_is_not_changed() {
        let display = "בעוד 150 מטר פנה שמאלה לרחוב הרצל".to_string();
        let _ = voice_text(&display);
        assert_eq!(display, "בעוד 150 מטר פנה שמאלה לרחוב הרצל");
        const { assert!(TABLE_SIZE > 300) };
    }

    #[test]
    fn street_names_get_niqqud() {
        let cases = [
            ("פנה ימינה לרחוב ז׳בוטינסקי", "פְּנֵה יָמִינָה לִרְחוֹב זַ'בּוֹטִינְסְקִי"),
            ("פנה שמאלה לשדרות רוטשילד", "פְּנֵה שְׂמֹאלָה לִשְׂדֵרוֹת רוֹטְשִׁילְד"),
            ("המשך ישר לרחוב בן גוריון", "הַמְשֵׁךְ יָשָׁר לִרְחוֹב בֶּן גּוּרְיוֹן"),
            ("צא לכיוון ירושלים", "צֵא לְכִוּוּן יְרוּשָׁלַיִם"),
            ("נסיעה לירושלים", "נסיעה לִירוּשָׁלַיִם"),
            ("לתל אביב", "לְתֵל אָבִיב"),
            ("פנה ימינה לרחוב אחד העם", "פְּנֵה יָמִינָה לִרְחוֹב אַחַד הָעָם"),
            ("לרחוב הנביאים", "לִרְחוֹב הַנְּבִיאִים"),
            ("שד׳ ירושלים", "שְׂדֵרוֹת יְרוּשָׁלַיִם"),
            ("לרחוב המלך דוד", "לִרְחוֹב הַמֶּלֶךְ דָּוִד"),
            ("ליפו", "לְיָפוֹ"),
            ("ודוד", "וְדָוִד"),
            ("ומשה", "וּמֹשֶׁה"),
            ("לבאר שבע", "לִבְאֵר שֶׁבַע"),
            ("ליהודה", "לִיהוּדָה"),
            ("לגבעת שמואל", "לְגִבְעַת שְׁמוּאֵל"),
            ("לאפרים", "לְאֶפְרַיִם"),
            ("לאליעזר", "לֶאֱלִיעֶזֶר"),
        ];
        for (display, voice) in cases {
            assert_eq!(voice_text(display), voice, "{display}");
        }
    }

    #[test]
    fn unknown_names_stay() {
        assert_eq!(voice_text("פנה ימינה לרחוב קלישר"), "פְּנֵה יָמִינָה לִרְחוֹב קלישר");
        // ה words are not glued to ל / ב (the article would be swallowed)
        assert_eq!(voice_text("להרצל"), "להרצל");
    }
}

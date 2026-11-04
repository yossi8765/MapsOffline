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

/// Number of vocalised entries (phrases + words), for the report / tests.
pub const TABLE_SIZE: usize = PHRASES.len() + WORDS.len();

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
    // word by word: a token is replaced only when it is exactly a table word (a vocalised phrase
    // word is no longer equal to the plain one, so it is not touched twice)
    let mut out = String::with_capacity(s.len() + 16);
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        if !word.is_empty() {
            match WORDS.iter().find(|(w, _)| *w == word.as_str()) {
                Some((_, v)) => out.push_str(v),
                None => out.push_str(word),
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
pub const SAMPLES: [&str; 3] = ["בעוד 300 מטר פנה ימינה לרחוב הרצל", "היכנס לכיכר וצא ביציאה השנייה לכביש 1", "מצלמת מהירות בעוד 400 מטר"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_get_niqqud() {
        assert_eq!(voice_text("בעוד 300 מטר פנה ימינה לרחוב הרצל"), "בְּעוֹד 300 מֶטֶר פְּנֵה יָמִינָה לִרְחוֹב הרצל");
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
        assert_eq!(voice_text("פנה ימינה לרחוב הרב בצלאל ז'ולטי"), "פְּנֵה יָמִינָה לִרְחוֹב הרב בצלאל ז'ולטי");
        assert_eq!(voice_text("צא ביציאה 5 לכיוון ירושלים, תל אביב"), "צֵא בַּיְּצִיאָה 5 לְכִוּוּן ירושלים, תל אביב");
        assert_eq!(voice_text("פנה ימינה לרחוב ישראלי"), "פְּנֵה יָמִינָה לִרְחוֹב ישראלי");
        assert_eq!(voice_text("Main Street"), "Main Street");
    }

    #[test]
    fn display_text_is_not_changed() {
        let display = "בעוד 150 מטר פנה שמאלה לרחוב הרצל".to_string();
        let _ = voice_text(&display);
        assert_eq!(display, "בעוד 150 מטר פנה שמאלה לרחוב הרצל");
        const { assert!(TABLE_SIZE > 60) };
    }
}

# MapsOffline · מפות אופליין

תוכנת מפות וניווט לישראל שפועלת **ללא אינטרנט**, ל-Windows. מתקין של ‎5 MB, הנתונים (כ-0.6 GB) יורדים פעם אחת או מועברים בחבילה למחשב לא מקוון.

Tauri 2 + Rust · React + MapLibre GL · PMTiles · SQLite FTS5.

## מה יש בפנים

- **מפה וקטורית** של ישראל (Protomaps/OSM) בעברית, אנגלית או ערבית; ערכות בהיר/כהה/אפור/לילה (לפי שקיעה); בניינים תלת-ממדיים; מחוות משטח מגע מהירות.
- **תצלום לוויין** (Sentinel-2) ו**טופוגרפיה** (הצללת שטח, פני שטח תלת-ממדיים, פרופיל גובה למסלול).
- **ניווט** ברכב, הליכה ואופניים עם חלופות והוראות בעברית; הימנעות מכבישי אגרה, משטחי A/B, מכבישים מהירים, מדרכי עפר ומפניות קשות. מקוון (OSRM) כשיש אינטרנט, מקומי תמיד.
- **תחבורה ציבורית** (GTFS של משרד התחבורה): תכנון מסע עם החלפות, תחנות במפה עם מספרי תחנה, זמני קו להיום ולוח יציאות לתחנה עם ספירה לאחור.
- **חיפוש** כתובות מדויקות (גם בכתיב מלא/חסר, שגיאות הקלדה ומקלדת בפריסה שגויה), יישובים, רחובות, מקומות לפי קטגוריה ("בית כנסת חניכי הישיבות"), תחנות לפי שם או מספר, קואורדינטות.
- **קטגוריות במפה**: בתי כנסת, ישיבות, תחנות דלק, בתי מרקחת ועוד כ-35 קטגוריות כסמנים.
- **מועדפים**: כתובות (בית/עבודה), קווים (לפי חברה, קו וכיוון) ותחנות; היסטוריה; "המיקום שלי" דרך שירות המיקום של Windows.
- **הדפסה** של מסלול עם כל ההוראות; ייצוא GPX/KML.
- **נתונים**: עדכון רכיבים (במקביל, עם השהיה וביטול לכל רכיב), בדיקת עדכונים, ייצוא/ייבוא חבילת נתונים דחוסה למחשב לא מקוון, עמידות בסינון תוכן (NetFree).

אפיון מלא: [docs/SPEC.md](docs/SPEC.md). ביצועים וגודל: [docs/PERF.md](docs/PERF.md), [docs/SIZE.md](docs/SIZE.md).

## התקנה

מורידים את `MapsOffline_<גרסה>_x64-setup.exe` מעמוד ה-Releases, מתקינים, ובלשונית **נתונים** לוחצים "עדכן הכול" (או מייבאים חבילת ‎.mapsdata שיוצאה במחשב מחובר). דרוש WebView2 (מובנה ב-Windows 10/11 מעודכנים).

## פיתוח

```
pnpm install
pnpm tauri dev          # הרצה עם טעינה חמה
pnpm tauri build        # מתקין ב-src-tauri/target/release/bundle/nsis
cd src-tauri && cargo test
```

כלי שורת פקודה לנתונים (בניית רכיבים, חיפוש, מסלול, לוחות זמנים, גובה):

```
cd src-tauri
cargo run --release --features cli --bin mapsoffline-data -- build --all
cargo run --release --features cli --bin mapsoffline-data -- search "הרצל 12 תל אביב"
cargo run --release --features cli --bin mapsoffline-data -- route --from 34.7818,32.0853 --to 35.2137,31.7683 --mode transit
cargo run --release --features cli --bin mapsoffline-data -- stop-board 25629
```

Skill לפיתוח (ארכיטקטורה, חוזה IPC, פקודות): `.claude/skills/mapsoffline/SKILL.md`.

## מקורות נתונים ורישיונות

- מפה, ניווט, כתובות ומקומות: [OpenStreetMap](https://www.openstreetmap.org) (ODbL) דרך [Protomaps](https://protomaps.com) ו-[Geofabrik](https://download.geofabrik.de).
- תחבורה ציבורית: GTFS של משרד התחבורה.
- תצלום לוויין: [Sentinel-2 cloudless](https://s2maps.eu) מאת EOX (CC BY-NC-SA 4.0, שימוש לא מסחרי).
- גובה: [Mapzen terrain tiles](https://registry.opendata.aws/terrain-tiles/) (AWS Open Data).
- ניווט מקוון: OSRM (FOSSGIS) ו-Transitous.

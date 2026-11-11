// Data sources and licences, one list for Settings → About ("מקורות ורישיונות"), the print
// sheet and anything else that has to credit them. Names stay in their own language.

export interface Credit {
  id: string;
  /** Who / what, as the source names itself. */
  name: string;
  /** i18n key of what the app uses it for. */
  use: CreditUse;
  licence: string;
  url: string;
}

export type CreditUse =
  | "creditUseMap"
  | "creditUseRouting"
  | "creditUseSearch"
  | "creditUseTransit"
  | "creditUseImagery"
  | "creditUseElevation"
  | "creditUseMinyanim"
  | "creditUseKohanim"
  | "creditUseDelays"
  | "creditUseOnlineRouting"
  | "creditUseLogos"
  | "creditUseFonts";

export const CREDITS: Credit[] = [
  { id: "osm", name: "© OpenStreetMap contributors", use: "creditUseMap", licence: "ODbL 1.0", url: "https://www.openstreetmap.org/copyright" },
  { id: "protomaps", name: "Protomaps", use: "creditUseMap", licence: "ODbL (data) · BSD-3 (basemap styles)", url: "https://protomaps.com" },
  { id: "geofabrik", name: "Geofabrik", use: "creditUseRouting", licence: "ODbL 1.0", url: "https://download.geofabrik.de" },
  { id: "datagov", name: "data.gov.il", use: "creditUseSearch", licence: "רישיון שימוש פתוח (data.gov.il)", url: "https://data.gov.il" },
  { id: "mot", name: "משרד התחבורה – GTFS", use: "creditUseTransit", licence: "רישיון שימוש פתוח", url: "https://gtfs.mot.gov.il" },
  { id: "govmap", name: "המרכז למיפוי ישראל (govmap)", use: "creditUseImagery", licence: "לשימוש אישי", url: "https://www.govmap.gov.il" },
  { id: "eox", name: "Sentinel-2 cloudless – EOX IT Services", use: "creditUseImagery", licence: "CC BY-NC-SA 4.0 (2018+)", url: "https://s2maps.eu" },
  { id: "mapterhorn", name: "Mapterhorn", use: "creditUseElevation", licence: "CC BY 4.0 / per source", url: "https://mapterhorn.com" },
  { id: "copernicus", name: "Copernicus GLO-30 (© DLR / Airbus)", use: "creditUseElevation", licence: "Copernicus licence", url: "https://spacedata.copernicus.eu" },
  { id: "nedarim", name: "נדרים פלוס – זמני תפילות", use: "creditUseMinyanim", licence: "באישור המקור", url: "https://www.nedarimplus.co.il" },
  { id: "hakohanim", name: "התאחדות טהרת כהנים", use: "creditUseKohanim", licence: "לפי תנאי ההתאחדות", url: "https://gps.hakohanim.org" },
  { id: "stride", name: "Open Bus Stride – הסדנא לידע ציבורי", use: "creditUseDelays", licence: "CC BY-SA 4.0", url: "https://open-bus-stride-api.hasadna.org.il" },
  { id: "osrm", name: "OSRM · FOSSGIS", use: "creditUseOnlineRouting", licence: "שירות ללא תשלום, שימוש לא מסחרי", url: "https://routing.openstreetmap.de" },
  { id: "transitous", name: "Transitous (MOTIS)", use: "creditUseOnlineRouting", licence: "שימוש לא מסחרי", url: "https://transitous.org" },
  { id: "operators", name: "לוגואים של מפעילי תחבורה", use: "creditUseLogos", licence: "public/operators/CREDITS.md", url: "" },
  { id: "noto", name: "Noto Sans Hebrew · Noto Sans", use: "creditUseFonts", licence: "SIL OFL 1.1", url: "https://fonts.google.com/noto" },
];

/** The short line under the map / on paper. */
export const SHORT_ATTRIBUTION = "© OpenStreetMap · Protomaps · מפ״י";

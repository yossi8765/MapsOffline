import {
  Baby,
  Banknote,
  BatteryCharging,
  BedDouble,
  Binoculars,
  BookOpen,
  Church,
  Clapperboard,
  Building2,
  Bus,
  Coffee,
  CreditCard,
  Croissant,
  Dog,
  Droplet,
  Droplets,
  Dumbbell,
  Flame,
  Fuel,
  GlassWater,
  GraduationCap,
  Hammer,
  HeartPulse,
  Hospital,
  Landmark,
  Mail,
  MoonStar,
  ParkingSquare,
  Pill,
  School,
  Scissors,
  Shield,
  ShieldAlert,
  Shirt,
  ShoppingBag,
  ShoppingCart,
  Stethoscope,
  Store,
  Tent,
  Toilet,
  Trees,
  FerrisWheel as PlaygroundIcon,
  Utensils,
  Waves,
  Wrench,
} from "lucide-static";
import type { I18nKey } from "./i18n";

/** Hand-drawn glyphs in the lucide style (24x24, 2px round strokes) for categories lucide lacks. */
const SVG_HEAD = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">';
/** A building with a Star of David inside. */
const Synagogue = `${SVG_HEAD}<path d="M3 21h18"/><path d="M5 21V8l7-5 7 5v13"/><path d="M12 9.4l3.5 6h-7z" stroke-width="1.5"/><path d="M12 18.2l-3.5-6h7z" stroke-width="1.5"/></svg>`;
/** A headstone. */
const Tombstone = `${SVG_HEAD}<path d="M4 21h16"/><path d="M7 21V9a5 5 0 0 1 10 0v12"/><path d="M9.5 13h5"/><path d="M9.5 16.5h5"/></svg>`;

export type PoiGroup = "religion" | "health" | "shopping" | "food" | "services" | "transport" | "education" | "leisure" | "nature" | "emergency";
export const POI_GROUPS: { id: PoiGroup; label: I18nKey }[] = [
  { id: "religion", label: "grpReligion" },
  { id: "transport", label: "grpTransport" },
  { id: "health", label: "grpHealth" },
  { id: "shopping", label: "grpShopping" },
  { id: "food", label: "grpFood" },
  { id: "services", label: "grpServices" },
  { id: "education", label: "grpEducation" },
  { id: "leisure", label: "grpLeisure" },
  { id: "nature", label: "grpNature" },
  { id: "emergency", label: "grpEmergency" },
];

/** A map-layer category: geocoder category ids, optional name filter, label, colour, glyph. */
export interface PoiCategory {
  id: string;
  group: PoiGroup;
  cats: string[];
  /** Keep only POIs whose name matches (e.g. yeshivot among jewish places of worship). */
  nameMatch?: RegExp;
  label: I18nKey;
  color: string;
  svg: string;
}

const C = {
  religion: "#4f46e5",
  health: "#e11d48",
  shopping: "#16a34a",
  food: "#dc2626",
  services: "#334155",
  transport: "#d97706",
  education: "#0369a1",
  leisure: "#9333ea",
  nature: "#15803d",
  emergency: "#b91c1c",
};

export const POI_CATEGORIES: PoiCategory[] = [
  // religion
  { id: "synagogue", group: "religion", cats: ["place_of_worship:jewish"], label: "catSynagogue", color: C.religion, svg: Synagogue },
  { id: "yeshiva", group: "religion", cats: ["place_of_worship:jewish", "college", "school"], nameMatch: /(?:^|[^\u05d0-\u05ea])(?:ישיב|מדרש|ביהמ|כולל(?![\u05d0-\u05ea]))/, label: "catYeshiva", color: "#6d28d9", svg: BookOpen },
  { id: "mikveh", group: "religion", cats: ["mikveh", "mikvah", "public_bath"], label: "catMikveh", color: "#0284c7", svg: Droplets },
  { id: "cemetery", group: "religion", cats: ["grave_yard", "cemetery"], label: "catCemetery", color: "#52525b", svg: Tombstone },
  { id: "mosque", group: "religion", cats: ["place_of_worship:muslim"], label: "catMosque", color: "#047857", svg: MoonStar },
  { id: "church", group: "religion", cats: ["place_of_worship:christian"], label: "catChurch", color: "#7c2d12", svg: Church },
  // transport
  { id: "fuel", group: "transport", cats: ["fuel"], label: "catFuel", color: C.transport, svg: Fuel },
  { id: "parking", group: "transport", cats: ["parking"], label: "catParking", color: "#2563eb", svg: ParkingSquare },
  { id: "charging", group: "transport", cats: ["charging_station"], label: "catCharging", color: "#0d9488", svg: BatteryCharging },
  { id: "busstation", group: "transport", cats: ["bus_station"], label: "catBusStation", color: "#0891b2", svg: Bus },
  { id: "carrepair", group: "transport", cats: ["car_repair", "car_wash", "tyres"], label: "catCarRepair", color: "#57534e", svg: Wrench },
  // health
  { id: "hospital", group: "health", cats: ["hospital"], label: "catHospital", color: C.health, svg: Hospital },
  { id: "clinic", group: "health", cats: ["clinic", "doctors"], label: "catClinic", color: "#be123c", svg: Stethoscope },
  { id: "pharmacy", group: "health", cats: ["pharmacy", "chemist"], label: "catPharmacy", color: "#059669", svg: Pill },
  { id: "dentist", group: "health", cats: ["dentist"], label: "catDentist", color: "#0e7490", svg: Stethoscope },
  { id: "vet", group: "health", cats: ["veterinary"], label: "catVet", color: "#a16207", svg: Dog },
  // shopping
  { id: "supermarket", group: "shopping", cats: ["supermarket"], label: "catSupermarket", color: C.shopping, svg: ShoppingCart },
  { id: "grocery", group: "shopping", cats: ["convenience", "greengrocer"], label: "catGrocery", color: "#15803d", svg: Store },
  { id: "mall", group: "shopping", cats: ["mall", "department_store"], label: "catMall", color: "#be185d", svg: ShoppingBag },
  { id: "clothes", group: "shopping", cats: ["clothes", "shoes"], label: "catClothes", color: "#db2777", svg: Shirt },
  { id: "hardware", group: "shopping", cats: ["hardware", "doityourself"], label: "catHardware", color: "#78350f", svg: Hammer },
  // food
  { id: "restaurant", group: "food", cats: ["restaurant", "fast_food"], label: "catFood", color: C.food, svg: Utensils },
  { id: "cafe", group: "food", cats: ["cafe"], label: "catCafe", color: "#92400e", svg: Coffee },
  { id: "bakery", group: "food", cats: ["bakery", "pastry"], label: "catBakery", color: "#b45309", svg: Croissant },
  // services
  { id: "bank", group: "services", cats: ["bank"], label: "catBank", color: C.services, svg: Banknote },
  { id: "atm", group: "services", cats: ["atm"], label: "catAtm", color: "#475569", svg: CreditCard },
  { id: "post", group: "services", cats: ["post_office"], label: "catPost", color: "#b91c1c", svg: Mail },
  { id: "police", group: "services", cats: ["police"], label: "catPolice", color: "#1d4ed8", svg: Shield },
  { id: "fire", group: "services", cats: ["fire_station"], label: "catFire", color: "#c2410c", svg: Flame },
  { id: "municipal", group: "services", cats: ["townhall", "community_centre"], label: "catMunicipal", color: "#0f766e", svg: Building2 },
  { id: "library", group: "services", cats: ["library"], label: "catLibrary", color: "#4338ca", svg: BookOpen },
  { id: "hairdresser", group: "services", cats: ["hairdresser", "beauty"], label: "catHairdresser", color: "#a21caf", svg: Scissors },
  { id: "toilets", group: "services", cats: ["toilets"], label: "catToilets", color: "#0e7490", svg: Toilet },
  { id: "drinkingwater", group: "services", cats: ["drinking_water"], label: "catDrinkingWater", color: "#0284c7", svg: GlassWater },
  // education
  { id: "school", group: "education", cats: ["school"], label: "catSchool", color: C.education, svg: School },
  { id: "kindergarten", group: "education", cats: ["kindergarten", "childcare"], label: "catKindergarten", color: "#0284c7", svg: Baby },
  { id: "university", group: "education", cats: ["university", "college"], label: "catUniversity", color: "#1e40af", svg: GraduationCap },
  // leisure & tourism
  { id: "hotel", group: "leisure", cats: ["hotel", "guest_house", "hostel", "motel"], label: "catHotel", color: C.leisure, svg: BedDouble },
  { id: "sights", group: "leisure", cats: ["attraction", "museum", "monument", "artwork"], label: "catSights", color: "#7e22ce", svg: Landmark },
  { id: "park", group: "leisure", cats: ["park", "garden"], label: "catPark", color: "#16a34a", svg: Trees },
  { id: "playground", group: "leisure", cats: ["playground"], label: "catPlayground", color: "#ca8a04", svg: PlaygroundIcon },
  { id: "cinema", group: "leisure", cats: ["cinema", "theatre"], label: "catCinema", color: "#be123c", svg: Clapperboard },
  { id: "beach", group: "leisure", cats: ["beach_resort", "beach", "swimming_pool", "water_park"], label: "catBeach", color: "#0ea5e9", svg: Waves },
  { id: "gym", group: "leisure", cats: ["fitness_centre", "sports_centre", "gym"], label: "catGym", color: "#ea580c", svg: Dumbbell },
  // nature & hiking
  { id: "spring", group: "nature", cats: ["spring"], label: "catSpring", color: "#0369a1", svg: Droplet },
  { id: "viewpoint", group: "nature", cats: ["viewpoint"], label: "catViewpoint", color: "#4d7c0f", svg: Binoculars },
  { id: "campsite", group: "nature", cats: ["camp_site"], label: "catCampSite", color: "#15803d", svg: Tent },
  // emergency & safety
  { id: "shelter", group: "emergency", cats: ["shelter"], label: "catShelter", color: "#7f1d1d", svg: ShieldAlert },
  { id: "defibrillator", group: "emergency", cats: ["defibrillator"], label: "catDefibrillator", color: "#dc2626", svg: HeartPulse },
];

/** The three categories pinned at the top of the picker. */
export const QUICK_CATEGORIES = ["synagogue", "yeshiva", "fuel"];

export function poiCategory(id: string): PoiCategory | undefined {
  return POI_CATEGORIES.find((c) => c.id === id);
}

/** Which of the *selected* categories a POI belongs to. Categories with a name filter (yeshivot
 *  among the synagogues) are tried first, so the icon no longer depends on the order the user
 *  picked them in. */
export function categoryOf(rawCat: string, name: string, selected: PoiCategory[]): PoiCategory | undefined {
  const base = rawCat.split(":")[0];
  const ordered = [...selected].sort((a, b) => Number(!!b.nameMatch) - Number(!!a.nameMatch));
  return ordered.find((c) => c.cats.some((x) => (x.includes(":") ? x === rawCat : x === base)) && (!c.nameMatch || c.nameMatch.test(name)));
}

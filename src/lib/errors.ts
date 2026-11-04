import { toApiError, type ApiError } from "./api";
import { translate, type I18nKey } from "./i18n";

export interface DescribedError {
  /** The user-facing sentence (translated) */
  title: string;
  /** Developer text for a "details" fold / copy button (English) */
  details?: string;
  /** The code has its own translated message (false: generic message + details) */
  known: boolean;
  code: string;
}

/** A blocked host named in the error (details, or "… <host> …" in the message). */
function blockedHost(err: ApiError): string {
  if (err.details && !/\s/.test(err.details.trim())) return err.details.trim();
  const m = /\b((?:[a-z0-9-]+\.)+[a-z]{2,})\b/i.exec(`${err.details ?? ""} ${err.message}`);
  return m ? m[1] : "";
}

/**
 * Any rejection as a translated message: every code maps to the i18n key `err_<CODE>`; an
 * unknown code gives "אירעה שגיאה" with the developer text as details.
 * `t` is the translator (store.t / useT()), or a language code.
 */
export function describeError(e: unknown, t: ((k: I18nKey) => string) | string): DescribedError {
  const tr = typeof t === "string" ? (k: I18nKey) => translate(t, k) : t;
  const err = toApiError(e);
  const key = `err_${err.code}` as I18nKey;
  const text = tr(key);
  const devText = err.details ? `${err.message}${err.message.includes(err.details) ? "" : ` (${err.details})`}` : err.message;
  // INTERNAL = unclassified: the generic message with the developer text is more useful
  if (err.code !== "INTERNAL" && text && text !== key) {
    const title = err.code === "BLOCKED_HOST" ? text.replace("{host}", blockedHost(err) || "?") : text;
    return { title, details: devText !== title ? devText : undefined, known: true, code: err.code };
  }
  return { title: tr("errGeneric"), details: devText, known: false, code: err.code };
}

/** One line for a toast: the translated message, or "אירעה שגיאה: <text>" for an unknown code. */
export function errorMessage(e: unknown, t: ((k: I18nKey) => string) | string): string {
  const d = describeError(e, t);
  return d.known || !d.details ? d.title : `${d.title}: ${d.details}`;
}

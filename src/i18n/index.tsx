import { createContext, useContext, useEffect, useMemo, type ReactNode } from "react";
import { en, type MessageKey } from "./en";
import { fr } from "./fr";
import { it } from "./it";

const catalogs = { en, it, fr } as const;
export type UiLang = keyof typeof catalogs;
export type { MessageKey };

/** Selectable app languages (native names in the picker). */
export const UI_LANG_OPTIONS: { code: UiLang; name: string }[] = [
  { code: "en", name: "English" },
  { code: "it", name: "Italiano" },
  { code: "fr", name: "Français" },
];

export function resolveUiLang(code: string | null | undefined): UiLang {
  const c = (code ?? "en").toLowerCase().slice(0, 2);
  return c in catalogs ? (c as UiLang) : "en";
}

export type TFunction = (key: MessageKey, vars?: Record<string, string | number>) => string;

export function translate(lang: UiLang, key: MessageKey, vars?: Record<string, string | number>): string {
  const table = catalogs[lang] ?? en;
  let s: string = table[key] ?? en[key] ?? key;
  if (vars) {
    for (const [k, v] of Object.entries(vars)) {
      s = s.split(`{${k}}`).join(String(v));
    }
  }
  return s;
}

type I18nValue = { lang: UiLang; t: TFunction };

const I18nCtx = createContext<I18nValue | null>(null);

export function I18nProvider({ lang, children }: { lang: string; children: ReactNode }) {
  const resolved = resolveUiLang(lang);
  const value = useMemo<I18nValue>(
    () => ({
      lang: resolved,
      t: (key, vars) => translate(resolved, key, vars),
    }),
    [resolved],
  );
  useEffect(() => {
    document.documentElement.lang = resolved;
  }, [resolved]);
  return <I18nCtx.Provider value={value}>{children}</I18nCtx.Provider>;
}

export function useI18n(): I18nValue {
  const ctx = useContext(I18nCtx);
  if (!ctx) throw new Error("useI18n outside I18nProvider");
  return ctx;
}

export function useT(): TFunction {
  return useI18n().t;
}

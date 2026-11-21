import { describe, expect, it } from "vitest";
import { en, type MessageKey } from "./en";
import { fr } from "./fr";
import { it as itCatalog } from "./it";
import { resolveUiLang, translate } from "./index";

describe("i18n catalogs", () => {
  it("it and fr cover every English key", () => {
    const keys = Object.keys(en) as MessageKey[];
    for (const k of keys) {
      expect(itCatalog[k], `missing it: ${k}`).toBeTruthy();
      expect(fr[k], `missing fr: ${k}`).toBeTruthy();
    }
  });

  it("resolves OS-like tags to shipped UI langs", () => {
    expect(resolveUiLang("it-IT")).toBe("it");
    expect(resolveUiLang("fr_FR")).toBe("fr");
    expect(resolveUiLang("de-DE")).toBe("en");
    expect(resolveUiLang(undefined)).toBe("en");
  });

  it("interpolates placeholders", () => {
    expect(translate("en", "update.toVersion", { version: "1.2.3" })).toBe("Update to v1.2.3");
    expect(translate("it", "update.toVersion", { version: "1.2.3" })).toContain("1.2.3");
    expect(translate("fr", "settings.langOverlay.uiLang")).toMatch(/application/i);
  });
});

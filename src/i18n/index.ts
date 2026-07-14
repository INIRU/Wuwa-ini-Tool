import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import type { AppLanguage } from "../contracts";
import en from "./en.json";
import ko from "./ko.json";

function detectLanguage(): AppLanguage {
  return navigator.language.toLowerCase().startsWith("ko") ? "ko" : "en";
}

void i18n.use(initReactI18next).init({
  resources: {
    en: { translation: en },
    ko: { translation: ko },
  },
  lng: detectLanguage(),
  fallbackLng: "en",
  interpolation: {
    escapeValue: false,
  },
});

export { i18n };

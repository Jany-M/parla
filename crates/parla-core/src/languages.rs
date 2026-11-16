//! One language table used by the translation prompt, the target picker and language detection.
use whatlang::Lang as W;

pub struct Language {
    pub code: &'static str,
    /// English name — the translation model wants full English names in its prompt.
    pub name: &'static str,
    detect: W,
}

pub const LANGUAGES: &[Language] = &[
    Language { code: "en", name: "English", detect: W::Eng },
    Language { code: "it", name: "Italian", detect: W::Ita },
    Language { code: "fr", name: "French", detect: W::Fra },
    Language { code: "de", name: "German", detect: W::Deu },
    Language { code: "es", name: "Spanish", detect: W::Spa },
    Language { code: "pt", name: "Portuguese", detect: W::Por },
    Language { code: "nl", name: "Dutch", detect: W::Nld },
    Language { code: "pl", name: "Polish", detect: W::Pol },
    Language { code: "ro", name: "Romanian", detect: W::Ron },
    Language { code: "cs", name: "Czech", detect: W::Ces },
    Language { code: "sv", name: "Swedish", detect: W::Swe },
    Language { code: "da", name: "Danish", detect: W::Dan },
    Language { code: "fi", name: "Finnish", detect: W::Fin },
    Language { code: "el", name: "Greek", detect: W::Ell },
    Language { code: "hu", name: "Hungarian", detect: W::Hun },
    Language { code: "bg", name: "Bulgarian", detect: W::Bul },
    Language { code: "hr", name: "Croatian", detect: W::Hrv },
    Language { code: "sk", name: "Slovak", detect: W::Slk },
    Language { code: "sl", name: "Slovenian", detect: W::Slv },
    Language { code: "ru", name: "Russian", detect: W::Rus },
    Language { code: "uk", name: "Ukrainian", detect: W::Ukr },
    Language { code: "tr", name: "Turkish", detect: W::Tur },
];

pub fn by_code(code: &str) -> Option<&'static Language> {
    LANGUAGES.iter().find(|l| l.code.eq_ignore_ascii_case(code))
}

/// Detection restricted to the supported set. Accepts whatlang's "reliable" verdict, or a
/// confident one on text long enough to mean something; `None` for short/ambiguous text.
pub fn detect(text: &str) -> Option<&'static str> {
    let alpha = text.chars().filter(|c| c.is_alphabetic()).count();
    if alpha < 12 {
        return None;
    }
    let allow: Vec<whatlang::Lang> = LANGUAGES.iter().map(|l| l.detect).collect();
    let info = whatlang::Detector::with_allowlist(allow).detect(text)?;
    if !(info.is_reliable() || (alpha >= 20 && info.confidence() >= MIN_CONFIDENCE)) {
        return None;
    }
    LANGUAGES.iter().find(|l| l.detect == info.lang()).map(|l| l.code)
}

const MIN_CONFIDENCE: f64 = 0.3;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_is_case_insensitive() {
        assert_eq!(by_code("IT").unwrap().name, "Italian");
        assert!(by_code("xx").is_none());
    }

    #[test]
    fn codes_are_unique() {
        let mut v: Vec<_> = LANGUAGES.iter().map(|l| l.code).collect();
        v.sort();
        v.dedup();
        assert_eq!(v.len(), LANGUAGES.len());
    }

    #[test]
    fn detects_the_three_working_languages() {
        assert_eq!(detect("Good morning everyone, thanks for joining the call today."), Some("en"));
        assert_eq!(detect("Buongiorno a tutti, grazie per essere intervenuti alla riunione di oggi."), Some("it"));
        assert_eq!(detect("Bonjour à tous, merci de vous être joints à la réunion aujourd'hui."), Some("fr"));
    }

    #[test]
    fn typical_meeting_sentences_are_detected_in_every_working_language() {
        for (text, code) in [
            ("Ask not what your country can do for you. Ask what you can do for your country.", "en"),
            ("We should move the release to next Thursday if the tests pass.", "en"),
            ("Buongiorno a tutti, iniziamo la riunione di oggi.", "it"),
            ("Penso che dovremmo spostare la consegna alla settimana prossima.", "it"),
            ("Bonjour à tous, nous allons commencer la réunion maintenant.", "fr"),
            ("Guten Morgen zusammen, wir beginnen jetzt mit dem Meeting.", "de"),
            ("Buenos días a todos, vamos a empezar la reunión de hoy.", "es"),
        ] {
            assert_eq!(detect(text), Some(code), "{text}");
        }
    }

    #[test]
    fn short_text_is_undetected() {
        assert_eq!(detect("ok"), None);
        assert_eq!(detect("yes sure"), None);
    }
}

//! The interface in English and French (US-8.4, NFR-14), with Fluent. The strings are in
//! `locales/*.ftl`, built into the binary; a message missing in French falls back to
//! English.

use std::sync::{Arc, LazyLock, RwLock};

pub use cb_core::Language;
use fluent_bundle::concurrent::FluentBundle;
pub use fluent_bundle::FluentArgs;
use fluent_bundle::FluentResource;

const EN: &str = include_str!("../locales/en.ftl");
const FR: &str = include_str!("../locales/fr.ftl");

type Bundle = FluentBundle<FluentResource>;

/// `language`, with `Auto` replaced by the system's language when there is a
/// translation for it.
pub fn resolve(language: Language) -> Language {
    match language {
        Language::Auto => {
            let system = sys_locale::get_locale().unwrap_or_default();
            if system.to_ascii_lowercase().starts_with("fr") {
                Language::Fr
            } else {
                Language::En
            }
        }
        other => other,
    }
}

fn bundle(language: Language) -> Bundle {
    let (tag, own) = match resolve(language) {
        Language::Fr => ("fr", FR),
        _ => ("en", EN),
    };
    let mut bundle = Bundle::new_concurrent(vec![tag.parse().expect("valid language tag")]);
    // egui would draw the bidi isolation marks around each value as boxes.
    bundle.set_use_isolating(false);
    let resource = |text: &str| FluentResource::try_new(text.to_owned()).expect("valid FTL");
    bundle
        .add_resource(resource(own))
        .expect("no duplicate message");
    if own != EN {
        // Messages already there are kept: only the missing ones come from English.
        let _ = bundle.add_resource(resource(EN));
    }
    bundle
}

static CURRENT: LazyLock<RwLock<Arc<Bundle>>> =
    LazyLock::new(|| RwLock::new(Arc::new(bundle(Language::En))));

/// Show the interface in `language` from the next frame on.
pub fn set_language(language: Language) {
    let bundle = Arc::new(bundle(language));
    *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = bundle;
}

fn format(bundle: &Bundle, id: &str, args: Option<&FluentArgs>) -> String {
    let Some(pattern) = bundle.get_message(id).and_then(|m| m.value()) else {
        return id.to_owned();
    };
    let mut errors = Vec::new();
    bundle
        .format_pattern(pattern, args, &mut errors)
        .into_owned()
}

/// The message `id` in the current language. Use [`tr!`](crate::tr).
pub fn text(id: &str, args: Option<&FluentArgs>) -> String {
    let bundle = CURRENT.read().unwrap_or_else(|e| e.into_inner()).clone();
    format(&bundle, id, args)
}

/// `tr!("id")`, or `tr!("id", name = value, …)` for a message with variables.
#[macro_export]
macro_rules! tr {
    ($id:literal) => {
        $crate::i18n::text($id, None)
    };
    ($id:literal, $($name:ident = $value:expr),+ $(,)?) => {{
        let mut args = $crate::i18n::FluentArgs::new();
        $(args.set(stringify!($name), $value);)+
        $crate::i18n::text($id, Some(&args))
    }};
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn ids(ftl: &str) -> BTreeSet<String> {
        ftl.lines()
            .filter(|l| l.chars().next().is_some_and(|c| c.is_ascii_lowercase()))
            .filter_map(|l| l.split_once(" =").map(|(id, _)| id.trim().to_owned()))
            .collect()
    }

    #[test]
    fn english_and_french_have_the_same_messages() {
        let (en, fr) = (ids(EN), ids(FR));
        assert!(!en.is_empty());
        assert_eq!(
            en.difference(&fr).collect::<Vec<_>>(),
            Vec::<&String>::new(),
            "missing in French"
        );
        assert_eq!(
            fr.difference(&en).collect::<Vec<_>>(),
            Vec::<&String>::new(),
            "only in French"
        );
        // Both parse and format without errors.
        for language in [Language::En, Language::Fr] {
            let bundle = bundle(language);
            for id in &en {
                let pattern = bundle.get_message(id).and_then(|m| m.value());
                assert!(pattern.is_some(), "{id} has no value");
            }
        }
    }

    /// Every `tr!("…")` in the sources names a message that exists.
    #[test]
    fn every_message_used_exists() {
        let en = ids(EN);
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut missing = Vec::new();
        for dir in ["crates/ui/src", "app/src"] {
            for entry in std::fs::read_dir(root.join(dir)).unwrap() {
                let path = entry.unwrap().path();
                if path.ends_with("i18n.rs") {
                    continue;
                }
                let source = std::fs::read_to_string(&path).unwrap();
                for (i, _) in source.match_indices("tr!(\"") {
                    let rest = &source[i + 5..];
                    let id = &rest[..rest.find('"').unwrap()];
                    if !en.contains(id) {
                        missing.push(format!("{}: {id}", path.display()));
                    }
                }
            }
        }
        assert!(missing.is_empty(), "{missing:#?}");
    }

    #[test]
    fn french_counts_take_the_right_form() {
        let fr = bundle(Language::Fr);
        let count = |n: usize| {
            let mut args = FluentArgs::new();
            args.set("count", n);
            format(&fr, "tray-title", Some(&args))
        };
        assert_eq!(count(1), "Tampon · 1 capture");
        assert_eq!(count(3), "Tampon · 3 captures");
    }
}

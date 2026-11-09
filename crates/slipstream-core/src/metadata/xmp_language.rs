//! Bounded BCP 47 language tag canonicalization for XMP language alternatives.

/// Grandfathered tags registered as BCP 47 exceptions (RFC 5646 Section 2.2.8).
const GRANDFATHERED_LANGUAGE_TAGS: [&str; 25] = [
    "en-gb-oed",
    "i-ami",
    "i-bnn",
    "i-default",
    "i-enochian",
    "i-hak",
    "i-klingon",
    "i-lux",
    "i-navajo",
    "i-pwn",
    "i-tao",
    "i-tay",
    "i-tsu",
    "sgn-be-fr",
    "sgn-be-nl",
    "sgn-ch-de",
    "art-lojban",
    "cel-gaulish",
    "no-bok",
    "no-nyn",
    "zh-guoyu",
    "zh-hakka",
    "zh-min",
    "zh-min-nan",
    "zh-xiang",
];

/// Canonicalizes a language tag to lowercase when it is well-formed BCP 47
/// (RFC 5646 syntactic grammar, including grandfathered tags); `x-default`
/// keeps its special XMP meaning.
pub(super) fn canonical_lang(lang: &str) -> Option<String> {
    if lang.eq_ignore_ascii_case("x-default") {
        return Some("x-default".into());
    }
    if lang.len() > 255 || !lang.is_ascii() {
        return None;
    }
    let lower = lang.to_ascii_lowercase();
    if GRANDFATHERED_LANGUAGE_TAGS.contains(&lower.as_str()) {
        return Some(lower);
    }
    let subtags: Vec<&str> = lower.split('-').collect();
    let alphabetic =
        |subtag: &str| !subtag.is_empty() && subtag.bytes().all(|b| b.is_ascii_alphabetic());
    let alphanumeric =
        |subtag: &str| !subtag.is_empty() && subtag.bytes().all(|b| b.is_ascii_alphanumeric());
    // privateuse-only tag: x followed by one or more 1*8alphanum subtags.
    if subtags[0].eq_ignore_ascii_case("x") {
        return (subtags.len() > 1
            && subtags[1..]
                .iter()
                .all(|subtag| (1..=8).contains(&subtag.len()) && alphanumeric(subtag)))
        .then_some(lower);
    }
    let mut position = 1;
    // language: 2*3ALPHA optionally followed by up to three extlang subtags,
    // 4ALPHA reserved for future use, or 5*8ALPHA registered.
    let language = subtags[0];
    if !alphabetic(language) {
        return None;
    }
    match language.len() {
        2 | 3 => {
            let mut extlangs = 0;
            while position < subtags.len()
                && extlangs < 3
                && subtags[position].len() == 3
                && alphabetic(subtags[position])
            {
                position += 1;
                extlangs += 1;
            }
        }
        4..=8 => {}
        _ => return None,
    }
    // script: 4ALPHA.
    if subtags
        .get(position)
        .is_some_and(|subtag| subtag.len() == 4 && alphabetic(subtag))
    {
        position += 1;
    }
    // region: 2ALPHA or 3DIGIT.
    if subtags.get(position).is_some_and(|subtag| {
        (subtag.len() == 2 && alphabetic(subtag))
            || (subtag.len() == 3 && subtag.bytes().all(|b| b.is_ascii_digit()))
    }) {
        position += 1;
    }
    // variants: 5*8alphanum or DIGIT 3alphanum, each unique.
    let mut variants = Vec::new();
    while subtags.get(position).is_some_and(|subtag| {
        ((5..=8).contains(&subtag.len()) && alphanumeric(subtag))
            || (subtag.len() == 4 && subtag.as_bytes()[0].is_ascii_digit() && alphanumeric(subtag))
    }) {
        let variant = subtags[position];
        if variants.contains(&variant) {
            return None;
        }
        variants.push(variant);
        position += 1;
    }
    // extensions: a singleton subtag (never x), each used at most once, followed
    // by one or more 2*8alphanum subtags.
    let mut singletons = [false; 128];
    while subtags.get(position).is_some_and(|subtag| {
        subtag.len() == 1 && !subtag.eq_ignore_ascii_case("x") && alphanumeric(subtag)
    }) {
        let singleton = usize::from(subtags[position].as_bytes()[0]);
        if singletons[singleton] {
            return None;
        }
        singletons[singleton] = true;
        position += 1;
        let mut extension = 0;
        while subtags
            .get(position)
            .is_some_and(|subtag| (2..=8).contains(&subtag.len()) && alphanumeric(subtag))
        {
            position += 1;
            extension += 1;
        }
        if extension == 0 {
            return None;
        }
    }
    // privateuse: x followed by one or more 1*8alphanum subtags.
    if subtags
        .get(position)
        .is_some_and(|subtag| subtag.eq_ignore_ascii_case("x"))
    {
        position += 1;
        let mut private_use = 0;
        while subtags
            .get(position)
            .is_some_and(|subtag| (1..=8).contains(&subtag.len()) && alphanumeric(subtag))
        {
            position += 1;
            private_use += 1;
        }
        if private_use == 0 {
            return None;
        }
    }
    (position == subtags.len()).then_some(lower)
}

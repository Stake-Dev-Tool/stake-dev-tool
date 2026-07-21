//! Share-link slug (subdomain label) rules and the default `word-word-nnn`
//! generator.

use rand::Rng;

/// Short, unambiguous words for generated slugs (no lookalike/offensive terms).
/// Two words + a 3-digit number keeps generated labels well under the 40-char
/// limit and reads cleanly as a subdomain.
const WORDS: &[&str] = &[
    "amber", "brave", "coral", "delta", "ember", "fable", "glide", "haze", "iris", "jade", "koala",
    "lunar", "maple", "noble", "ocean", "pearl", "quartz", "raven", "sage", "tidal", "umber",
    "vivid", "willow", "xenon", "yarrow", "zephyr", "cobalt", "dusk", "flint", "grove", "hazel",
    "indigo", "juniper", "kelp", "lotus", "moss", "nimbus", "onyx", "pine", "reef", "slate",
    "topaz", "vapor", "wren", "birch", "cedar", "opal", "spark",
];

/// A subdomain label: `^[a-z0-9][a-z0-9-]{0,38}[a-z0-9]$`, or a single alnum
/// char. 1–40 chars, lowercase alphanumerics + hyphens, no leading/trailing
/// hyphen.
pub(crate) fn is_valid_label(slug: &str) -> bool {
    let len = slug.len();
    if len == 0 || len > 40 {
        return false;
    }
    let is_alnum = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit();
    if !slug.chars().all(|c| is_alnum(c) || c == '-') {
        return false;
    }
    slug.chars().next().is_some_and(is_alnum) && slug.chars().last().is_some_and(is_alnum)
}

/// Generate a `word-word-nnn` slug candidate. The caller retries on collision.
pub(crate) fn generate() -> String {
    let mut rng = rand::thread_rng();
    let a = WORDS[rng.gen_range(0..WORDS.len())];
    let b = WORDS[rng.gen_range(0..WORDS.len())];
    let n = rng.gen_range(0..1000);
    format!("{a}-{b}-{n:03}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_rules() {
        for good in [
            "a",
            "ab",
            "abc",
            "sweet-demo",
            "a1b2",
            "x9",
            "a".repeat(40).as_str(),
        ] {
            assert!(is_valid_label(good), "{good} should be valid");
        }
        for bad in [
            "",
            "-abc",
            "abc-",
            "AbC",
            "a_b",
            "a b",
            "a.b",
            "-",
            "a".repeat(41).as_str(),
        ] {
            assert!(!is_valid_label(bad), "{bad} should be invalid");
        }
    }

    #[test]
    fn generated_slugs_are_valid_labels() {
        for _ in 0..200 {
            let slug = generate();
            assert!(is_valid_label(&slug), "generated {slug} should be valid");
        }
    }
}

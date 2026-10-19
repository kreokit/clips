//! Word error rate, the accuracy measure for our transcription fixtures.

/// (substitutions + deletions + insertions) / reference word count, after lowercasing and
/// stripping punctuation. Returns 0.0 for two empty texts.
pub fn word_error_rate(reference: &str, hypothesis: &str) -> f64 {
    let reference = normalize(reference);
    let hypothesis = normalize(hypothesis);
    if reference.is_empty() {
        return if hypothesis.is_empty() { 0.0 } else { 1.0 };
    }

    // Levenshtein distance over words, one row at a time.
    let mut prev: Vec<usize> = (0..=hypothesis.len()).collect();
    for (i, r) in reference.iter().enumerate() {
        let mut row = vec![i + 1; hypothesis.len() + 1];
        for (j, h) in hypothesis.iter().enumerate() {
            let substitution = prev[j] + usize::from(r != h);
            row[j + 1] = substitution.min(prev[j + 1] + 1).min(row[j] + 1);
        }
        prev = row;
    }
    prev[hypothesis.len()] as f64 / reference.len() as f64
}

fn normalize(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|w| {
            w.chars()
                .filter(|c| c.is_alphanumeric() || *c == '\'')
                .flat_map(char::to_lowercase)
                .collect::<String>()
        })
        .filter(|w| !w.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_text_ignoring_case_and_punctuation() {
        assert_eq!(word_error_rate("Hello, world!", "hello world"), 0.0);
    }

    #[test]
    fn counts_substitutions_deletions_and_insertions() {
        let reference = "the quick brown fox";
        assert_eq!(word_error_rate(reference, "the quick red fox"), 0.25);
        assert_eq!(word_error_rate(reference, "the brown fox"), 0.25);
        assert_eq!(word_error_rate(reference, "the very quick brown fox"), 0.25);
        assert_eq!(word_error_rate(reference, ""), 1.0);
    }

    #[test]
    fn handles_non_latin_scripts() {
        assert_eq!(word_error_rate("Добар ден!", "добар ден"), 0.0);
        assert_eq!(
            word_error_rate("Mirëdita, si jeni?", "mirëdita si jeni"),
            0.0
        );
    }

    #[test]
    fn empty_reference() {
        assert_eq!(word_error_rate("", ""), 0.0);
        assert_eq!(word_error_rate("", "noise"), 1.0);
    }
}

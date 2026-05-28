//! Twitter / X intent URL + suggested tweet text for `qk card`.
//!
//! Pure formatters — the command prints them after writing the PNG. No
//! network calls; the URL just primes the user's tweet compose box when
//! they click it.

/// The suggested copy. Pretty-printed (with newlines) so users can copy it
/// straight from the terminal. The intent URL embeds the same text
/// URL-encoded.
pub fn tweet_text() -> &'static str {
    "My iPhone, in one image.\n\nGenerated with `qk card` #quokka\ngithub.com/dutradotdev/quokka"
}

/// Twitter "intent/tweet" URL with the suggested text URL-encoded. Posting
/// to X via `intent/tweet` still works and degrades to a "compose" page
/// when not logged in.
pub fn tweet_intent_url() -> String {
    format!(
        "https://twitter.com/intent/tweet?text={}",
        urlencoding::encode(tweet_text())
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intent_url_encodes_the_text() {
        let url = tweet_intent_url();
        assert!(url.starts_with("https://twitter.com/intent/tweet?text="));
        // Hash sign must be encoded so Twitter doesn't drop the fragment.
        assert!(url.contains("%23quokka"));
        // Newlines too.
        assert!(url.contains("%0A"));
    }
}

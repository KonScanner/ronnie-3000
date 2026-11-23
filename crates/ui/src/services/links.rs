//! The one rule for what the system browser may open.

use eframe::egui;

/// Whether `url` is a web page: `http` or `https` in any case, and then a host.
///
/// The system browser also accepts `file:` and other schemes, so data from a server must pass
/// this check first.
#[must_use]
pub(crate) fn is_web_url(url: &str) -> bool {
    let after_scheme = ["https://", "http://"].into_iter().find_map(|scheme| {
        let start = url.get(..scheme.len())?;
        start
            .eq_ignore_ascii_case(scheme)
            .then(|| &url[scheme.len()..])
    });
    after_scheme.is_some_and(|rest| {
        !rest.is_empty()
            && !rest.starts_with('/')
            && !rest
                .chars()
                .any(|character| character.is_whitespace() || character.is_control())
    })
}

/// Opens a page from external data in the system browser. Any other URL does nothing.
pub(crate) fn open_web_url(context: &egui::Context, url: String) {
    if is_web_url(&url) {
        context.open_url(egui::OpenUrl::new_tab(url));
    }
}

#[cfg(test)]
mod tests {
    use super::is_web_url;

    #[test]
    fn only_a_web_page_with_a_host_passes() {
        for url in [
            "https://www.fixtures.invalid/tones/1",
            "http://127.0.0.1:8080/",
            "HTTPS://EXAMPLE.INVALID",
            "HtTp://example.invalid/?a=b",
        ] {
            assert!(is_web_url(url), "{url}");
        }
        for url in [
            "",
            "file:///etc/passwd",
            "FILE:///etc/passwd",
            "javascript:alert(1)",
            "ssh://example.invalid",
            "//example.invalid",
            "https",
            "https:",
            "https:/example.invalid",
            "https://",
            "https:///etc/passwd",
            " https://example.invalid",
            "https://example.invalid/a b",
            "https://example.invalid/\n--flag",
            "éhttps://example.invalid",
        ] {
            assert!(!is_web_url(url), "{url}");
        }
    }
}

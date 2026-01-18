use url::Url;

// Attention: The behavior of this function may differ from your expectations. See the tests below for examples.
//
// Url::make_relative() is the best we have, though it's tricky. But it's suits our needs now.
pub fn validate_url_base(base: &Url, url: &Url) -> bool {
    base.make_relative(url).map(|relative| {
        relative.split(['/', '?', '#']).next().unwrap() != ".."
    }).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest(base, url, result,
        case("https://example.com", "https://example.com", true),
        case("https://example.com", "https://example.com/", true),
        case("https://example.com/", "https://example.com", true),

        case("https://example.com", "https://www.example.com", false),
        case("https://www.example.com", "https://example.com", false),
        case("https://example.com", "http://example.com", false),
        case("http://example.com", "https://example.com", false),

        // Attention here:
        case("https://example.com/not-a-dir", "https://example.com/", true),
        case("https://example.com/dir/", "https://example.com/", false),
        case("https://example.com/not-a-dir", "https://example.com/page", true),
        case("https://example.com/dir/", "https://example.com/page", false),

        case("https://example.com", "https://example.com/page", true),
        case("https://example.com", "https://example.com/dir/page", true),
        case("https://example.com/sitemap.xml", "https://example.com/page", true),
        case("https://example.com/sitemap.xml", "https://example.com/dir/page", true),

        case("https://example.com/dir", "https://example.com/dir/page", true),
        case("https://example.com/dir/", "https://example.com/dir/page", true),
        case("https://example.com/dir", "https://example.com/dir/page/inner", true),
        case("https://example.com/dir/", "https://example.com/dir/page/inner", true),
        case("https://example.com/dir/sitemap.xml", "https://example.com", false),
        case("https://example.com/dir/sitemap.xml", "https://example.com/page", false),
        case("https://example.com/dir/sitemap.xml", "https://example.com/dir/page", true),
        case("https://example.com/dir/sitemap.xml", "https://example.com/dir/page/inner", true),
        case("https://example.com/dir/sitemap.xml", "https://example.com/dir/page/inner?a", true),
        case("https://example.com/dir/sitemap.xml", "https://example.com/dir/page?a", true),
        case("https://example.com/dir/sitemap.xml", "https://example.com/dir/?a", true),
        case("https://example.com/dir/sitemap.xml", "https://example.com/dir?a", false),
        case("https://example.com/dir/sitemap.xml", "https://example.com/?a", false),
    )]
    fn url_base_validation(base: &str, url: &str, result: bool) {
        let base = base.parse().unwrap();
        let url = url.parse().unwrap();
        assert_eq!(validate_url_base(&base, &url), result)
    }
}
use url::Url;

// Attention: The behavior of this function may differ from your expectations. See the tests below for examples.
//
// Url::make_relative() is the best we have, though it's tricky. But it's suits our needs now.
pub fn validate_url_base(base: &Url, url: &Url) -> Option<String> {
    base.make_relative(url).filter(|relative| {
        relative.split(['/', '?', '#']).next().unwrap() != ".."
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest(base, url, result,
        case("https://example.com", "https://example.com", Some("")),
        case("https://example.com", "https://example.com/", Some("")),
        case("https://example.com/", "https://example.com", Some("")),

        case("https://example.com", "https://www.example.com", None),
        case("https://www.example.com", "https://example.com", None),
        case("https://example.com", "http://example.com", None),
        case("http://example.com", "https://example.com", None),

        case("https://example.com/not-a-dir", "https://example.com/", Some("/")),
        case("https://example.com/dir/", "https://example.com/", None),
        case("https://example.com/not-a-dir", "https://example.com/page", Some("page")),
        case("https://example.com/dir/", "https://example.com/page", None),

        case("https://example.com", "https://example.com/page", Some("page")),
        case("https://example.com", "https://example.com/dir/page", Some("dir/page")),
        case("https://example.com/sitemap.xml", "https://example.com/page", Some("page")),
        case("https://example.com/sitemap.xml", "https://example.com/dir/page", Some("dir/page")),

        case("https://example.com/dir", "https://example.com/dir/page", Some("dir/page")),
        case("https://example.com/dir/", "https://example.com/dir/page", Some("page")),
        case("https://example.com/dir", "https://example.com/dir/page/inner", Some("dir/page/inner")),
        case("https://example.com/dir/", "https://example.com/dir/page/inner", Some("page/inner")),
        case("https://example.com/dir/sitemap.xml", "https://example.com", None),
        case("https://example.com/dir/sitemap.xml", "https://example.com/page", None),
        case("https://example.com/dir/sitemap.xml", "https://example.com/dir/page", Some("page")),
        case("https://example.com/dir/sitemap.xml", "https://example.com/dir/page/inner", Some("page/inner")),
        case("https://example.com/dir/sitemap.xml", "https://example.com/dir/page/inner?a", Some("page/inner?a")),
        case("https://example.com/dir/sitemap.xml", "https://example.com/dir/page?a", Some("page?a")),
        case("https://example.com/dir/sitemap.xml", "https://example.com/dir/?a", Some("/?a")),
        case("https://example.com/dir/sitemap.xml", "https://example.com/dir?a", None),
        case("https://example.com/dir/sitemap.xml", "https://example.com/?a", None),
    )]
    fn url_base_validation(base: &str, url: &str, result: Option<&str>) {
        let base = base.parse().unwrap();
        let url = url.parse().unwrap();
        assert_eq!(validate_url_base(&base, &url).as_deref(), result)
    }
}
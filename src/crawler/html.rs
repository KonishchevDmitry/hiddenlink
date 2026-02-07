use std::collections::HashSet;

use log::{Level, log_enabled, trace};
use scraper::{Html, HtmlTreeSink};
use tendril::TendrilSink;
use url::Url;

use crate::core::GenericResult;

use super::util;

pub const SIZE_LIMIT: u64 = 1024 * 1024;

pub fn find_resources(page_url: &Url, data: &[u8], filter: &Url) -> GenericResult<impl Iterator<Item=Url>> {
    // Html::parse_document() can't parse bytes - only strings are supported, so parse it manually
    let parser = html5ever::parse_document(HtmlTreeSink::new(Html::new_document()), Default::default());
    let document = parser.from_utf8().one(data);

    if !document.errors.is_empty() && log_enabled!(Level::Trace) {
        trace!("Got the following errors during HTML page parsing:");
        for error in &document.errors {
            trace!("* {error}");
        }
    }

    let mut resources = HashSet::new();
    let mut filtered_out = HashSet::new();

    trace!("Resource finder:");

    // There are a lot of cases which we may not cover here, but we don't need to be so precise
    for element in document.root_element().descendent_elements() {
        for attribute in ["href", "src"] {
            if let Some(link) = element.attr(attribute) && !link.is_empty() {
                if let Some(url) = parse_resource_link(page_url, link, Some(filter)) && &url != page_url {
                    if resources.insert(url.clone()) {
                        trace!("* Found: {url}");
                    }
                } else if log_enabled!(Level::Trace) && filtered_out.insert(link) {
                    trace!("* Ignoring: {link}");
                }
            }
        }
    }

    Ok(resources.into_iter())
}

fn parse_resource_link(base: &Url, link: &str, filter: Option<&Url>) -> Option<Url> {
    if link.is_empty() {
        return None;
    }

    let mut url = Url::options().base_url(Some(base)).parse(link).ok()?;

    if let Some(filter) = filter {
        if !util::validate_url_base(filter, &url) {
            return None;
        }
    }

    url.set_fragment(None);
    Some(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest(link, expected, local,
        case("https://example.com/", Some("https://example.com/"), true),
        case("https://example.com/category/", Some("https://example.com/category/"), true),
        case("https://example.com/category/article/", Some("https://example.com/category/article/"), true),
        case("https://example.com/category/other-article/", Some("https://example.com/category/other-article/"), true),
        case("https://example.com/other-category/", Some("https://example.com/other-category/"), true),
        case("https://github.com/KonishchevDmitry/hiddenlink", Some("https://github.com/KonishchevDmitry/hiddenlink"), false),

        case("/assets/style.css", Some("https://example.com/assets/style.css"), true),
        case("assets/style.css", Some("https://example.com/category/article/assets/style.css"), true),
        case("../style.css", Some("https://example.com/category/style.css"), true),
        case("//cdn.com/jquery.js", Some("https://cdn.com/jquery.js"), false),

        case("ссылка", Some("https://example.com/category/article/%D1%81%D1%81%D1%8B%D0%BB%D0%BA%D0%B0"), true),
        case("#anchor", Some("https://example.com/category/article/"), true),
        case("page?a=b&c=d#anchor", Some("https://example.com/category/article/page?a=b&c=d"), true),

        case("", None, false),
        case("about:blank", Some("about:blank"), false),
        case("mailto:user@example.com", Some("mailto:user@example.com"), false),
        case("data:image/png;base64,...", Some("data:image/png;base64,..."), false),
        case("browser://version", Some("browser://version"), false),
        case("chrome-extension://extension/pages/options.html", Some("chrome-extension://extension/pages/options.html"), false),
    )]
    fn resource_link_parsing(link: &str, expected: Option<&str>, local: bool) {
        let filter = Url::parse("https://example.com/").unwrap();
        let base = Url::parse("https://example.com/category/article/").unwrap();

        let result = parse_resource_link(&base, link, None);
        assert_eq!(result.as_ref().map(|url| url.as_str()), expected);

        let filtered_result = parse_resource_link(&base, link, Some(&filter));
        assert_eq!(filtered_result.as_ref().map(|url| url.as_str()), local.then(|| expected.unwrap()));
    }
}
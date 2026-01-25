use log::debug;
use reqwest::Client;
use reqwest::redirect::{Attempt, Policy};
use reqwest::header::HeaderMap;
use url::Url;

use crate::core::GenericResult;
use crate::transport::http;

use super::util;

pub fn new_client(base_url: &Url) -> GenericResult<Client> {
    let base_url = base_url.clone();

    let default_redirect_policy = Policy::default();
    let secured_redirect_policy = move |attempt: Attempt| {
        debug!("The crawling request to {} was redirected to {}.",
            attempt.previous().last().unwrap(), attempt.url());

        if !util::validate_url_base(&base_url, attempt.url()) {
            let url = attempt.url().clone();
            return attempt.error(format!(
                "got a redirect to {url} which is outside of the base URL ({base_url})"))
        }

        default_redirect_policy.redirect(attempt)
    };

    Ok(Client::builder()
        .no_proxy()
        .hickory_dns(true)
        .https_only(true)
        .tls_backend_native()
        .http1_title_case_headers()
        .default_headers(get_default_headers()?)
        .user_agent(format!("{}/{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")))
        .brotli(true).deflate(true).gzip(true).zstd(true) // Browsers send Accept-Encoding header by default, so do we
        .redirect(Policy::custom(secured_redirect_policy))
        .timeout(http::CONNECTION_TIMEOUT)
        .pool_idle_timeout(None) // We want to manually control TCP connection lifetime
        .build()?
    )
}

fn get_default_headers() -> GenericResult<HeaderMap> {
    let headers = [
        // We don't actually need any of these headers in our requests, but we want our requests to have a typical size
        // to not look too suspicious. These are typical headers of Chrome browser.

        // Use different header name to not collide with our User-Agent which we want to be real
        ("User-Agent-Chrome", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/144.0.0.0 Safari/537.36"),

        ("sec-ch-ua", r#""Not(A:Brand";v="8", "Chromium";v="144", "Google Chrome";v="144""#),
        ("sec-ch-ua-mobile", "?0"),
        ("sec-ch-ua-platform", r#""macOS""#),
        ("Upgrade-Insecure-Requests", "1"),

        ("Sec-Fetch-Site", "none"),
        ("Sec-Fetch-Mode", "navigate"),
        ("Sec-Fetch-User", "?1"),
        ("Sec-Fetch-Dest", "document"),

        ("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8,application/signed-exchange;v=b3;q=0.7"),
        ("Accept-Language", "en-GB,en-US;q=0.9,en;q=0.8"),
    ];

    let mut map = HeaderMap::new();
    for (name, value) in headers {
        let value = value.parse().map_err(|_| format!(
            "invalid {name} header value"))?;

        if map.insert(name, value).is_some() {
            return Err!("duplicate header: {name}");
        }
    }

    Ok(map)
}
use std::str::FromStr;

use async_trait::async_trait;
use mime::Mime;
use rand::seq::SliceRandom;
use reqwest::{header, Response};
use url::Url;

use crate::core::GenericResult;

use super::{Crawler, Delay};
use super::sitemap;
use super::util;

#[async_trait]
pub trait Resource: Send + Sync {
    fn name(&self) -> &'static str;
    async fn process(&self, crawler: &mut Crawler, response: Response) -> GenericResult<u64>;
}

pub struct Sitemap {
    url: Url,
}

impl Sitemap {
    pub fn new(url: Url) -> Sitemap {
        Sitemap { url }
    }
}

#[async_trait]
impl Resource for Sitemap {
    fn name(&self) -> &'static str {
        "sitemap"
    }

    async fn process(&self, crawler: &mut Crawler, response: Response) -> GenericResult<u64> {
        let sitemap_url = response.url().clone();
        if !util::validate_url_base(&self.url, &sitemap_url) {
            return Err!("got an unexpected sitemap redirect: {} -> {}", self.url, sitemap_url);
        }

        let data = read_response("sitemap", response, sitemap::SIZE_LIMIT).await?;

        match sitemap::parse(&data)? {
            sitemap::Sitemap::Index(index) => {
                let mut inner_sitemaps = index.sitemaps;
                inner_sitemaps.shuffle(&mut rand::rng());

                for inner_sitemap in inner_sitemaps {
                    if util::validate_url_base(&sitemap_url, &inner_sitemap.location) {
                        crawler.add(inner_sitemap.location.clone(), None, Sitemap::new(inner_sitemap.location));
                    } else {
                        crawler.on_error(format_args!(
                            "Sitemap {sitemap_url} refers an invalid inner sitemap URL {}",
                            inner_sitemap.location));
                    }
                }
            },

            sitemap::Sitemap::UrlSet(sitemap) => {
                let mut entries = sitemap.urls;
                entries.shuffle(&mut rand::rng());

                for entry in entries {
                    if util::validate_url_base(&sitemap_url, &entry.location) {
                        crawler.add(entry.location, Some(Delay::Page), Page::new());
                    } else {
                        crawler.on_error(format_args!("Sitemap {sitemap_url} refers an invalid URL {}", entry.location));
                    }
                }
            },
        }

        Ok(data.len() as u64)
    }
}

struct Page {
}

impl Page {
    fn new() -> Page {
        Page {}
    }
}

#[async_trait]
impl Resource for Page {
    fn name(&self) -> &'static str {
        "page"
    }

    async fn process(&self, crawler: &mut Crawler, response: Response) -> GenericResult<u64> {
        match get_content_type(&response) {
            Ok(content_type) if content_type.type_() == mime::TEXT && content_type.subtype() == mime::HTML => {
                // FIXME(konishchev): Crawl page resources
                Asset::new().process(crawler, response).await
            },
            Ok(content_type) => {
                crawler.on_error(format_args!("Got an unexpected content type for {} page: {content_type}", response.url()));
                Asset::new().process(crawler, response).await
            },
            Err(err) => {
                crawler.on_error(format_args!("Failed to process {} page: {err}", response.url()));
                Asset::new().process(crawler, response).await
            },
        }
    }
}

struct Asset {
}

impl Asset {
    fn new() -> Asset {
        Asset {}
    }
}

#[async_trait]
impl Resource for Asset {
    fn name(&self) -> &'static str {
        "asset"
    }

    async fn process(&self, _crawler: &mut Crawler, mut response: Response) -> GenericResult<u64> {
        let mut size = 0;

        // XXX(konishchev): Size limit
        while let Some(chunk) = response.chunk().await? {
            size += chunk.len() as u64;
        }

        Ok(size)
    }
}

fn get_content_type(response: &Response) -> GenericResult<Mime> {
    let value = response.headers().get(header::CONTENT_TYPE)
        .ok_or("Content-Type header is missing")?;

    let content_type = value.to_str().ok()
        .and_then(|value| Mime::from_str(value).ok())
        .ok_or_else(|| format!("invalid Content-Type: {value:?}"))?;

    Ok(content_type)
}

async fn read_response(name: &str, mut response: Response, size_limit: u64) -> GenericResult<Vec<u8>> {
    let mut data = Vec::new();

    if let Some(size) = response.content_length() {
        if size > size_limit {
            return Err!("{name} size limit is exceeded");
        }
        data.reserve_exact(size as usize);
    }

    while let Some(chunk) = response.chunk().await? {
        if (data.len() + chunk.len()) as u64 > size_limit {
            return Err!("{name} size limit is exceeded");
        }
        data.extend_from_slice(&chunk);
    }

    Ok(data)
}
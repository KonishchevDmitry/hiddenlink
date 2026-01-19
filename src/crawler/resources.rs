use async_trait::async_trait;
use reqwest::Response;
use url::Url;

use crate::core::GenericResult;

use super::{CrawlQueue, Delay};
use super::sitemap;
use super::util;

#[async_trait]
pub trait Resource: Send + Sync {
    fn name(&self) -> &'static str;
    async fn process(&self, response: Response, queue: &mut CrawlQueue) -> GenericResult<u64>;
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

    async fn process(&self, mut response: Response, queue: &mut CrawlQueue) -> GenericResult<u64> {
        let sitemap_url = response.url().clone();
        if !util::validate_url_base(&self.url, &sitemap_url) {
            return Err!("got an unexpected sitemap redirect: {} -> {}", self.url, sitemap_url);
        }

        let mut data = Vec::new();
        if let Some(size) = response.content_length() {
            if size > sitemap::SIZE_LIMIT {
                return Err!("sitemap size limit is exceeded");
            }
            data.reserve_exact(size as usize);
        }

        while let Some(chunk) = response.chunk().await? {
            if (data.len() + chunk.len()) as u64 > sitemap::SIZE_LIMIT {
                return Err!("sitemap size limit is exceeded");
            }
            data.extend_from_slice(&chunk);
        }

        match sitemap::parse(&data)? {
            sitemap::Sitemap::Index(index) => {
                for inner_sitemap in index.sitemaps {
                    if util::validate_url_base(&sitemap_url, &inner_sitemap.location) {
                        queue.add(inner_sitemap.location.clone(), None, Sitemap::new(inner_sitemap.location));
                    } else {
                        queue.on_error(format_args!(
                            "Sitemap {sitemap_url} refers an invalid inner sitemap URL {}",
                            inner_sitemap.location));
                    }
                }
            },

            sitemap::Sitemap::UrlSet(sitemap) => {
                for entry in sitemap.urls {
                    if util::validate_url_base(&sitemap_url, &entry.location) {
                        queue.add(entry.location, Some(Delay::Page), Page::new());
                    } else {
                        queue.on_error(format_args!("Sitemap {sitemap_url} refers an invalid URL {}", entry.location));
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

    async fn process(&self, response: Response, _queue: &mut CrawlQueue) -> GenericResult<u64> {
        // FIXME(konishchev): Rewrite
        let data = response.bytes().await?;
        Ok(data.len() as u64)
    }
}
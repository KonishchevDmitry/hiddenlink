use async_trait::async_trait;
use reqwest::Response;

use crate::core::GenericResult;

use super::{sitemap, CrawlQueue, CrawlTask};

#[async_trait]
pub trait Resource: Send + Sync {
    fn name(&self) -> &'static str;
    async fn process(&self, response: Response, queue: &mut CrawlQueue) -> GenericResult<u64>;
}

pub struct Sitemap {
}

impl Sitemap {
    pub fn new() -> Sitemap {
        Sitemap {}
    }
}

#[async_trait]
impl Resource for Sitemap {
    fn name(&self) -> &'static str {
        "sitemap"
    }

    async fn process(&self, response: Response, queue: &mut CrawlQueue) -> GenericResult<u64> {
        // FIXME(konishchev): Rewrite
        let data = response.bytes().await?;
        let sitemap = sitemap::parse(&data)?;

        match sitemap {
            sitemap::Sitemap::Index(index) => {
                for sitemap in index.sitemaps {
                    queue.add(CrawlTask::new(sitemap.location, Sitemap::new()));
                }
            },
            sitemap::Sitemap::UrlSet(sitemap) => {
                for entry in sitemap.urls {
                    queue.add(CrawlTask::new(entry.location, Page::new()));
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
#![allow(dead_code)] // FIXME(konishchev): Drop it
// Creates an additional noise by crawling the peer to mask the tunnel connections among real HTTPS connections

mod resources;
mod sitemap;
mod util;

use std::collections::VecDeque;
use std::fmt;

use log::{debug, warn};
use rand::Rng;
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use tokio::time::{self, Duration};
use url::Url;

use crate::core::GenericResult;

use self::resources::{Resource, Sitemap};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrawlerConfig {
    #[serde(with = "humantime_serde")]
    pub max_period: Duration,

    #[serde(with = "humantime_serde")]
    pub max_delay: Duration,
}

pub struct Crawler {
    sitemap: Url,
    max_period: Duration,
    max_delay: Duration,

    client: Client, // FIXME(konishchev): Limit pool lifetime
    queue: VecDeque<CrawlTask>,
}

impl Crawler {
    pub fn new(sitemap: &Url, config: &CrawlerConfig) -> GenericResult<Crawler> {
        let client = Client::builder()
            .user_agent(format!("{}/{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")))
            // FIXME(konishchev): native-tls-vendored?
            // .tls_backend_native()
            .http1_title_case_headers()
            // FIXME(konishchev): Custom resolver or hickory-dns?
            // .dns_resolver()
            // Browsers send Accept-Encoding header by default, so do we
            // FIXME(konishchev): Compare default headers
            .brotli(true).deflate(true).gzip(true).zstd(true)
            // FIXME(konishchev): Derive from browser settings
            // .timeout(timeout).read_timeout(timeout).connect_timeout(timeout)
            .no_proxy()
            .https_only(true) // FIXME(konishchev): Enable?
            .build()?;

        Ok(Crawler {
            sitemap: sitemap.clone(),
            max_period: config.max_period,
            max_delay: config.max_delay,

            client,
            queue: VecDeque::new(),
        })
    }

    pub async fn run(&mut self) {
        loop {
            let task = self.queue.pop_front().unwrap_or_else(|| {
                CrawlTask::new(self.sitemap.clone(), Some(Delay::Crawl), Sitemap::new(self.sitemap.clone()))
            });

            if let Some(delay_spec) = task.delay {
                let max_delay = match delay_spec {
                    Delay::Crawl => self.max_period,
                    Delay::Page => self.max_delay,
                };

                let delay = rand::rng().random_range(Duration::ZERO..=max_delay);
                debug!("Delaying {} {} crawling by {delay:.0?}...", task.resource.name(), task.url);

                time::sleep(delay).await;
            }

            debug!("Crawling {} {}...", task.resource.name(), task.url);

            let queue_size_before = self.queue.len();
            match self.process(&task).await {
                Ok(size) => debug!("Fetched {}.", humansize::format_size(size, humansize::BINARY)),
                Err(err) => self.on_error(format_args!("Failed to fetch {}: {err}", task.url)),
            }

            let queue_size_after = self.queue.len();
            if queue_size_after > queue_size_before {
                debug!("{} tasks have been added to the crawl queue.", queue_size_after - queue_size_before);
            }
        }
    }

    async fn process(&mut self, task: &CrawlTask) -> GenericResult<u64> {
        let response = self.client.get(task.url.clone()).send().await?;

        let url = response.url();
        if url != &task.url {
            debug!("The server redirected {} to {}.", task.url, url);
        }

        let status = response.status();
        if status != StatusCode::OK {
            return Err!("the server returned {status} status code");
        }

        task.resource.process(self, response).await
    }

    // FIXME(konishchev): Validate urls
    // FIXME(konishchev): Deduplicate + limit?
    fn add<R: Resource + 'static>(&mut self, url: Url, delay: Option<Delay>, resource: R) {
        self.queue.push_back(CrawlTask::new(url, delay, resource));
    }

    // FIXME(konishchev): Metrics
    // FIXME(konishchev): Request time metrics
    fn on_error(&mut self, message: fmt::Arguments) {
        warn!("{message}.");
    }
}

#[derive(Clone, Copy)]
enum Delay {
    Crawl,
    Page,
}

struct CrawlTask {
    url: Url,
    delay: Option<Delay>,
    resource: Box<dyn Resource>,
}

impl CrawlTask {
    fn new<R: Resource + 'static>(url: Url, delay: Option<Delay>, resource: R) -> CrawlTask {
        CrawlTask {
            url,
            delay,
            resource: Box::new(resource),
        }
    }
}
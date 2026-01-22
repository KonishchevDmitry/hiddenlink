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
use validator::Validate;

use crate::core::GenericResult;

use self::resources::{Resource, Sitemap};

#[derive(Clone, Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct CrawlerConfig {
    #[serde(with = "humantime_serde")]
    pub max_period: Duration,

    #[serde(with = "humantime_serde")]
    pub max_delay: Duration,

    #[validate(range(min = 1))]
    pub max_client_capacity: Option<usize>,
}

pub struct Crawler {
    sitemap: Url,
    config: CrawlerConfig,

    client: Option<LimitedClient>,
    queue: VecDeque<CrawlTask>,
}

impl Crawler {
    pub fn new(sitemap: &Url, config: &CrawlerConfig) -> GenericResult<Crawler> {
        Ok(Crawler {
            sitemap: sitemap.clone(),
            config: config.clone(),

            client: None,
            queue: VecDeque::new(),
        })
    }

    pub async fn run(&mut self) {
        loop {
            let task = self.queue.pop_front().unwrap_or_else(|| {
                // Even when client capacity is not configured, we don't want to use a single connection all the time,
                // so reset it at least on each scrape start.
                if self.client.take().is_some() {
                    debug!("Drop previous crawler client due to new crawling iteration.");
                }
                CrawlTask::new(self.sitemap.clone(), Some(Delay::Crawl), Sitemap::new(self.sitemap.clone()))
            });

            if let Some(delay_spec) = task.delay {
                let max_delay = match delay_spec {
                    Delay::Crawl => self.config.max_period,
                    Delay::Page => self.config.max_delay,
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

    async fn process(&mut self, task: &CrawlTask) -> GenericResult<u64> {
        let cost = task.delay.map(|_| 1).unwrap_or_default();
        let response = self.client(cost)?.get(task.url.clone()).send().await?;

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

    fn client<'a>(&'a mut self, cost: usize) -> GenericResult<Client> {
        if let Some(limited_client) = self.client.as_mut() {
            let Some(capacity) = limited_client.capacity else {
                return Ok(limited_client.client.clone());
            };

            // We want zero cost requests to reuse current client
            limited_client.used_capacity += cost;
            if limited_client.used_capacity <= capacity {
                return Ok(limited_client.client.clone());
            }

            debug!("Crawler client has reached its capacity ({capacity}). Recreate it.");
        }

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

        self.client.replace(LimitedClient {
            client: client.clone(),
            capacity: self.config.max_client_capacity.map(|capacity| {
                rand::rng().random_range(1..=capacity)
            }),
            used_capacity: cost,
        });

        Ok(client)
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

struct LimitedClient {
    client: Client,
    capacity: Option<usize>,
    used_capacity: usize,
}
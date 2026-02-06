// Creates an additional noise by crawling the peer to mask the tunnel connections among real HTTPS connections.
//
// In 2026 browsers behave as follows:
// 1. Open a single HTTP/2 connection.
// 2. Keep it open around one minute when idle.
//
// For this reason we don't implement multiple crawlers now – it seems that it won't make any better for us.

mod client;
mod html;
mod metrics;
mod resources;
mod sitemap;
mod util;

use std::collections::VecDeque;
use std::error::Error;
use std::fmt;
use std::sync::Arc;
use std::time::Instant;

use log::{debug, info, warn};
use rand::Rng;
use reqwest::{Client, StatusCode};
use rustls::pki_types::DnsName;
use serde::Deserialize;
use tokio::time::{self, Duration};
use url::Url;
use validator::Validate;

use crate::core::GenericResult;

pub use self::metrics::CrawlerMetrics;
use self::resources::{Resource, Sitemap};

#[derive(Clone, Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct CrawlerConfig {
    #[validate(length(min = 1))]
    pub domain: String,

    #[validate(length(min = 1))]
    pub sitemap_path: String,

    #[serde(with = "humantime_serde")]
    pub max_period: Duration,

    #[serde(with = "humantime_serde")]
    pub max_delay: Duration,

    #[validate(range(min = 1))]
    pub max_client_capacity: Option<usize>,
}

pub struct Crawler {
    base: Url,
    sitemap: Url,
    config: CrawlerConfig,

    queue: VecDeque<CrawlTask>,
    client: Option<LimitedClient>,
    pub metrics: Arc<CrawlerMetrics>
}

impl Crawler {
    pub fn new(config: &CrawlerConfig) -> GenericResult<Crawler> {
        let mut base = Url::parse("https://localhost/")?; // url crate has no URL builder

        DnsName::try_from(config.domain.as_str()).ok().and_then(|domain| {
            base.set_host(Some(domain.as_ref())).ok()
        }).ok_or_else(|| format!("Invalid domain name: {:?}", config.domain))?;

        let sitemap = base.join(&config.sitemap_path).map_err(|_| format!(
            "Invalid sitemap path: {:?}", config.sitemap_path))?;

        Ok(Crawler {
            base,
            sitemap,
            config: config.clone(),

            queue: VecDeque::new(),
            client: None,
            metrics: Arc::new(CrawlerMetrics::new()),
        })
    }

    pub async fn run(&mut self) {
        info!("The crawler is started for {}.", self.sitemap);

        loop {
            let task = self.queue.pop_front().unwrap_or_else(|| {
                // Even when client capacity is not configured, we don't want to use a single connection all the time,
                // so reset it at least on each scrape start.
                if self.client.take().is_some() {
                    self.metrics.active_connections.dec();
                    debug!("Drop previous crawler client due to new crawling iteration.");
                }
                CrawlTask::new(self.sitemap.clone(), Some(Delay::Crawl), Sitemap::new(self.sitemap.clone()))
            });

            if let Some(delay_spec) = task.delay {
                // We want to have periods with no open TCP connection
                if let Some(ref client) = self.client &&
                    let Some(capacity) = client.capacity && client.used_capacity >= capacity {
                    debug!("Crawler client has reached its capacity ({capacity}). Close it.");
                    self.metrics.active_connections.dec();
                    self.client = None;
                }

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
    // FIXME(konishchev): Deduplicate (redirects?) + limit?
    fn add<R: Resource + 'static>(&mut self, url: Url, delay: Option<Delay>, resource: R) {
        self.queue.push_back(CrawlTask::new(url, delay, resource));
    }

    async fn process(&mut self, task: &CrawlTask) -> GenericResult<u64> {
        let url = &task.url;
        if !util::validate_url_base(&self.base, url) {
            return Err!("an attempt to crawl outside of the base url: {url}");
        }

        let cost = task.delay.map(|_| 1).unwrap_or_default();
        let request = self.client(cost)?.get(url.clone());

        let start_time = Instant::now();
        let result = request.send().await;
        self.metrics.request_times.observe(start_time.elapsed().as_secs_f64());

        let response = result.map_err(|err| {
            let err = err.without_url();

            // reqwest/hyper errors hide all details, so extract the underlying error
            let mut err: &dyn Error = &err;
            while let Some(source) = err.source() {
                err = source;
            }

            err.to_string()
        })?;

        let status = response.status();
        if status != StatusCode::OK {
            return Err!("the server returned {status} status code");
        }

        task.resource.process(self, response).await
    }

    fn client(&mut self, cost: usize) -> GenericResult<Client> {
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
            self.metrics.active_connections.dec();
            self.client = None;
        }

        let limited_client = self.client.insert(LimitedClient {
            client: client::new_client(&self.base)?,
            capacity: self.config.max_client_capacity.map(|capacity| {
                rand::rng().random_range(1..=capacity)
            }),
            used_capacity: cost,
        });

        // Please note that it actually doesn't guarantee TCP connection creation, but we don't have an access to
        // client's pool, so expose as much information as we can now.
        self.metrics.new_connections.inc();
        self.metrics.active_connections.inc();

        Ok(limited_client.client.clone())
    }

    fn on_error(&mut self, message: fmt::Arguments) {
        self.metrics.errors.inc();
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

struct LimitedClient {
    client: Client,
    capacity: Option<usize>,
    used_capacity: usize,
}
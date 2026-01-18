#![allow(dead_code)] // FIXME(konishchev): Drop it
// Creates an additional noise by crawling the peer to mask the tunnel connections among real HTTPS connections

mod resources;
mod sitemap;

use std::collections::VecDeque;

use log::{debug, warn};
use reqwest::Client;
use url::Url;

use crate::core::GenericResult;

use self::resources::{Resource, Sitemap};

pub struct Crawler {
    sitemap: Url,
    client: Client, // FIXME(konishchev): Limit pool lifetime
    queue: CrawlQueue,
}

impl Crawler {
    pub fn new(sitemap: &Url) -> GenericResult<Crawler> {
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
            client,
            queue: CrawlQueue::new(),
        })
    }

    pub async fn run(&mut self) {
        // FIXME(konishchev): Infinite loop with delays
        self.queue.add(CrawlTask::new(self.sitemap.clone(), Sitemap::new()));

        while let Some(task) = self.queue.next() {
            debug!("Crawling {} {}...", task.resource.name(), task.url);
            match self.process(&task).await {
                Ok(size) => debug!("Fetched {}.", humansize::format_size(size, humansize::BINARY)),
                Err(err) => warn!("Failed to fetch {}: {err}.", task.url)
            }
        }
    }

    async fn process(&mut self, task: &CrawlTask) -> GenericResult<u64> {
        // FIXME(konishchev): Add checks
        let response = self.client.get(task.url.clone()).send().await?;
        task.resource.process(response, &mut self.queue).await
    }
}

struct CrawlTask {
    url: Url,
    resource: Box<dyn Resource>,
}

impl CrawlTask {
    fn new<R: Resource + 'static>(url: Url, resource: R) -> CrawlTask {
        CrawlTask {
            url,
            resource: Box::new(resource),
        }
    }
}

struct CrawlQueue {
    queue: VecDeque<CrawlTask>,
}

impl CrawlQueue {
    fn new() -> CrawlQueue {
        CrawlQueue {
            queue: VecDeque::new(),
        }
    }

    fn add(&mut self, task: CrawlTask) {
        self.queue.push_back(task);
    }

    fn next(&mut self) -> Option<CrawlTask> {
        self.queue.pop_front()
    }
}
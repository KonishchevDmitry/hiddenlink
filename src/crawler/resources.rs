use std::str::FromStr;
use std::time::Duration;

use async_trait::async_trait;
use log::{trace, error};
use mime::Mime;
use mini_moka::sync::Cache;
use rand::seq::SliceRandom;
use reqwest::{header, Response};
use url::Url;

use crate::core::{EmptyResult, GenericResult};

use super::{Crawler, Delay};
use super::html::{self, ResourceSource};
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

        let data = read_response(self.name(), response, sitemap::SIZE_LIMIT).await?;

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

    // XXX(konishchev): Update resource type cache?
    async fn process(&self, crawler: &mut Crawler, response: Response) -> GenericResult<u64> {
        match get_content_type(&response) {
            Ok(content_type) if content_type.type_() == mime::TEXT && content_type.subtype() == mime::HTML => {},
            Ok(content_type) => {
                crawler.on_error(format_args!(
                    "Got an unexpected content type for {} {}: {content_type}",
                    self.name(), response.url()));
                return Asset::new().process(crawler, response).await
            },
            Err(err) => {
                crawler.on_error(format_args!("Failed to process {} {}: {err}", self.name(), response.url()));
                return Asset::new().process(crawler, response).await
            },
        }

        let page_url = response.url().clone();
        let data = read_response(self.name(), response, html::SIZE_LIMIT).await?;

        for (url, source) in html::find_resources(&page_url, &data, &crawler.base) {
            let resource_type = match crawler.resource_type_cache.get(&url) {
                Ok(Some(resource_type)) => {
                    trace!("{url} is known as {resource_type}.");
                    resource_type
                },
                Ok(None) => {
                    let resource_type = ResourceType::guess_from_source(source);
                    trace!("Guessing {url} as {resource_type}.");
                    resource_type
                },
                Err(err) => {
                    // XXX(konishchev): Don't skip
                    crawler.on_error(format_args!("Failed to process {url}: {err}"));
                    continue;
                },
            };

            // FIXME(konishchev): Deduplicate + add right after
            #[allow(dead_code)]
            if false {
                crawler.add(url, resource_type.delay(), UnknownResource::new());
            }
        }

        Ok(data.len() as u64)
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

struct UnknownResource {
}

impl UnknownResource {
    fn new() -> UnknownResource {
        UnknownResource {}
    }
}

#[async_trait]
impl Resource for UnknownResource {
    fn name(&self) -> &'static str {
        "resource"
    }

    async fn process(&self, crawler: &mut Crawler, response: Response) -> GenericResult<u64> {
        let url = response.url();

        let (resource_type, resource): (_, Box<dyn Resource>) = match get_content_type(&response) {
            Ok(content_type) if content_type.type_() == mime::TEXT && content_type.subtype() == mime::HTML => {
                (ResourceType::Page, Box::new(Page::new()))
            },
            Ok(_) => {
                (ResourceType::Asset, Box::new(Asset::new()))
            },
            Err(err) => {
                crawler.on_error(format_args!("Failed to process {} {url}: {err}", self.name()));
                (ResourceType::Asset, Box::new(Asset::new()))
            },
        };

        if let Err(err) = crawler.resource_type_cache.add(url, resource_type) {
            error!("Failed to cache resource type of {url}: {err}.");
        }

        resource.process(crawler, response).await
    }
}

#[derive(Clone, Copy, strum::Display)]
#[strum(serialize_all = "kebab-case")]
enum ResourceType {
    Page,
    Asset,
}

impl ResourceType {
    fn guess_from_source(source: ResourceSource) -> ResourceType {
        match source {
            ResourceSource::Image => ResourceType::Asset,
            ResourceSource::Link => ResourceType::Page,
            ResourceSource::Other => ResourceType::Asset,
        }
    }

    fn delay(self) -> Option<Delay> {
        match self {
            ResourceType::Page => Some(Delay::Page),
            ResourceType::Asset => None,
        }
    }
}

pub struct ResourceTypeCache {
    base: Url,
    cache: Cache<String, ResourceType>,
}

impl ResourceTypeCache {
    pub fn new(base: &Url) -> ResourceTypeCache {
        ResourceTypeCache {
            base: base.to_owned(),
            cache: Cache::builder()
                // FIXME(konishchev): Select values
                .max_capacity(1000)
                .time_to_idle(Duration::from_secs(60 * 60))
                .build(),
        }
    }

    fn add(&mut self, url: &Url, resource_type: ResourceType) -> EmptyResult {
        self.cache.insert(self.key(url)?, resource_type);
        Ok(())
    }

    fn get(&mut self, url: &Url) -> GenericResult<Option<ResourceType>> {
        Ok(self.cache.get(&self.key(url)?))
    }

    fn key(&self, url: &Url) -> GenericResult<String> {
        Ok(self.base.make_relative(url).ok_or_else(|| format!(
            "invalid resource URL for the current base ({})", self.base))?)
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
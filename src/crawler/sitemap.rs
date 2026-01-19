#[cfg(test)] use std::include_bytes;
use std::io::Cursor;

#[cfg(test)] use itertools::Itertools;
use serde::Deserialize;
use url::Url;

use crate::core::GenericResult;

#[derive(Deserialize)]
pub enum Sitemap {
    #[serde(rename = "urlset")]
    UrlSet(UrlSet),
    #[serde(rename = "sitemapindex")]
    Index(Index),
}

#[derive(Deserialize)]
pub struct UrlSet {
    #[serde(rename = "url")]
    pub urls: Vec<UrlEntry>,
}

#[derive(Deserialize)]
pub struct UrlEntry {
    #[serde(rename = "loc")]
    pub location: Url,
}

#[derive(Deserialize)]
pub struct Index {
    #[serde(rename = "sitemap")]
    pub sitemaps: Vec<SitemapEntry>,
}

#[derive(Deserialize)]
pub struct SitemapEntry {
    #[serde(rename = "loc")]
    pub location: Url,
}

// The limit is defined by the standard (https://www.sitemaps.org/protocol.html)
pub const SIZE_LIMIT: u64 = 50 * 1024 * 1024;

// According to https://www.sitemaps.org/protocol.html, sitemap can also be an Atom/RSS or a plain text file + also can
// be gzip compressed. For now don't support these variations.
pub fn parse(buf: &[u8]) -> GenericResult<Sitemap> {
    Ok(quick_xml::de::from_reader(Cursor::new(buf))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_urlset() {
        let sitemap = parse(include_bytes!("testdata/sitemap-urlset.xml")).unwrap();

        let Sitemap::UrlSet(url_set) = sitemap else {
            unreachable!()
        };

        assert_eq!(url_set.urls.iter().map(|entry| entry.location.as_str()).collect_vec(), vec![
            "http://www.example.com/",
            "http://www.example.com/catalog?item=12&desc=vacation_hawaii",
            "http://www.example.com/catalog?item=73&desc=vacation_new_zealand",
            "http://www.example.com/catalog?item=74&desc=vacation_newfoundland",
            "http://www.example.com/catalog?item=83&desc=vacation_usa",
        ]);
    }

    #[test]
    fn parse_index() {
        let sitemap = parse(include_bytes!("testdata/sitemap-index.xml")).unwrap();

        let Sitemap::Index(index) = sitemap else {
            unreachable!()
        };

        assert_eq!(index.sitemaps.iter().map(|entry| entry.location.as_str()).collect_vec(), vec![
            "http://www.example.com/sitemap1.xml",
            "http://www.example.com/sitemap2.xml",
        ]);
    }
}
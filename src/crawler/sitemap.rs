// XXX(konishchev): Drop it
#![allow(dead_code)]

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
    // XXX(konishchev): This location can be a Sitemap, an Atom file, RSS file or a simple text file.
    // XXX(konishchev): Compressed gzip
    // XXX(konishchev): The location of a Sitemap file determines the set of URLs that can be included in that Sitemap. A Sitemap file located at http://example.com/catalog/sitemap.xml can include any URLs starting with http://example.com/catalog/ but can not include URLs starting with http://example.com/images/.
    #[serde(rename = "loc")]
    pub location: Url,
}

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
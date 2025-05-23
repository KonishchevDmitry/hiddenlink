use std::fmt::Write;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use itertools::Itertools;
use log::{debug, warn};
use rustls::{RootCertStore, ServerConfig};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use serde_derive::{Serialize, Deserialize};
use validator::Validate;
use x509_parser::extensions::{ParsedExtension, GeneralName};
use x509_parser::nom::AsBytes;

use crate::core::GenericResult;

#[derive(Serialize, Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct TlsDomainConfig {
    cert: PathBuf,
    key: PathBuf,
}

pub struct TlsDomains {
    pub default_domain: TlsDomain,
    additional_domains: Vec<TlsDomain>,
}

impl TlsDomains {
    pub fn new(default_domain: &TlsDomainConfig, additional_domains: &[TlsDomainConfig]) -> GenericResult<TlsDomains> {
        Ok(TlsDomains {
            default_domain: TlsDomain::new(&default_domain.cert, &default_domain.key)?,
            additional_domains: additional_domains.iter().map(|config| {
                TlsDomain::new(&config.cert, &config.key)
            }).collect::<Result<Vec<_>, _>>()?,
        })
    }

    pub fn select(&self, requested_domain: Option<&str>) -> &TlsDomain {
        let mut best_match = None;

        if let Some(requested_domain) = requested_domain {
            for domain in &self.additional_domains {
                for base in &domain.domains {
                    if match_domains(base, requested_domain) {
                        let depth = base.chars().filter(|c| *c == '.').count();
                        match best_match {
                            Some((_, best_depth)) if best_depth >= depth => {},
                            _ => {
                                best_match.replace((domain, depth));
                            },
                        }
                    }
                }
            }
        }

        best_match.map(|best_match| best_match.0).unwrap_or(&self.default_domain)
    }
}

pub struct TlsDomain {
    domains: Vec<String>,
    config: Arc<ServerConfig>,
}

impl TlsDomain {
    fn new(cert_path: &Path, key_path: &Path) -> GenericResult<TlsDomain> {
        let (domains, config) = load_certs(cert_path, key_path)?;
        Ok(TlsDomain {domains, config})
    }

    pub fn primary_domain(&self) -> &str {
        self.domains.first().unwrap()
    }

    pub fn get_config(&self, requested_domain: Option<&str>) -> (&str, Arc<ServerConfig>) {
        if let Some(requested) = requested_domain {
            if let Some(domain) = self.domains.iter().find(|&domain| domain == requested) {
                return (domain, self.config.clone());
            }
        }
        (self.primary_domain(), self.config.clone())
    }
}

fn match_domains(base: &str, domain: &str) -> bool {
    if domain == base {
        return true
    }

    if domain.ends_with(base) {
        let pos = domain.len() - base.len();
        if domain.get(pos-1..pos) == Some(".") {
            return true;
        }
    }

    false
}

pub fn load_roots() -> GenericResult<RootCertStore> {
    let native_certs = rustls_native_certs::load_native_certs();

    if !native_certs.errors.is_empty() {
        let mut message = String::from("Got the following errors during certificate loading:");
        for err in native_certs.errors {
            write!(&mut message, "\n* {err}").unwrap();
        }
        warn!("{message}");
    }

    let mut roots = RootCertStore::empty();
    for cert in native_certs.certs {
        roots.add(cert)?;
    }

    Ok(roots)
}

fn load_certs(cert_path: &Path, key_path: &Path) -> GenericResult<(Vec<String>, Arc<ServerConfig>)> {
    let (domains, certs) = load_cert(cert_path).map_err(|e| format!(
        "Failed to load certificate {:?}: {}", cert_path, e))?;

    let key = load_key(key_path).map_err(|e| format!(
        "Failed to load private key {:?}: {}", key_path, e))?;

    let config = Arc::new(ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| format!("Failed to load private key: {}", e))?);

    Ok((domains, config))
}

fn load_cert(path: &Path) -> GenericResult<(Vec<String>, Vec<CertificateDer<'static>>)> {
    let mut file = BufReader::new(File::open(path)?);
    let certs = rustls_pemfile::certs(&mut file).collect::<Result<Vec<_>, _>>()?;

    let mut domains = None;

    debug!("Loading {}:", path.display());

    for cert in &certs {
        let (_, cert) = x509_parser::parse_x509_certificate(cert.as_bytes())?;

        let mut common_names = cert.subject().iter_common_name();

        let common_name = common_names
            .next().ok_or("Got a certificate without Common Name")?
            .as_str().ok().filter(|name| !name.is_empty()).ok_or("Got a certificate with an invalid Common Name")?;

        if common_names.next().is_some() {
            return Err!("Got a certificate with multiple Common Names");
        }

        if cert.is_ca() {
            debug!("* CA: {}", common_name);
            continue;
        }

        let mut names = Vec::new();

        for extension in cert.extensions() {
            if let ParsedExtension::SubjectAlternativeName(name) = extension.parsed_extension() {
                for name in &name.general_names {
                    match name {
                        GeneralName::DNSName(name)
                            // To be able to simply calculate domain depth via dots count
                            if !name.starts_with('.') && !name.ends_with('.') && !name.contains("..") => {
                            names.push((*name).to_owned());
                        }
                        _ => return Err!("Got an unsupported Subject Alternative Name: {:?}", name),
                    }
                }
            }
        }

        debug!("* {} ({})", common_name, names.iter().join(", "));

        if names.is_empty() {
            return Err!("The certificate has no Subject Alternative Name");
        }

        match names.iter().position(|name| name == common_name) {
            Some(index) => {
                let name = names.remove(index);
                names.insert(0, name);
            },
            None => return Err!("Certificate's Common Name doesn't match Subject Alternative Name"),
        }

        if domains.replace(names).is_some() {
            return Err!("The file contains multiple certificates");
        }
    }

    Ok((
        domains.ok_or("The file doesn't contain any certificate")?,
        certs,
    ))
}

fn load_key(path: &Path) -> GenericResult<PrivateKeyDer<'static>> {
    let mut file = BufReader::new(File::open(path)?);

    let mut key = Option::None;
    while let Some(item) = rustls_pemfile::private_key(&mut file)? {
        if key.replace(item).is_some() {
            return Err!("the file contains more than one private key")
        }
    }

    Ok(key.ok_or("the file doesn't contain any private key")?)
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    #[rstest(base, domain, result,
        case("some.domain", "some.domain", true),
        case("some.domain", "other.domain", false),
        case("some.domain", "sub.some.domain", true),
        case("some.domain", "sub.sub.some.domain", true),
    )]
    fn match_domains(base: &str, domain: &str, result: bool) {
        assert_eq!(super::match_domains(base, domain), result)
    }
}
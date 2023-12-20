use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use itertools::Itertools;
use log::error;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use serde_derive::{Serialize, Deserialize};
use tokio_rustls::TlsAcceptor;
use validator::Validate;

use crate::core::GenericResult;

#[derive(Serialize, Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct TlsDomainConfig {
    cert: PathBuf,
    key: PathBuf,
}

pub struct TlsDomains {
    server_name: String,
    default_domain: TlsDomain,
    additional_domains: Vec<(String, TlsDomain)>,
}

impl TlsDomains {
    pub fn new(server_name: &str, default_domain: &TlsDomainConfig, additional_domains: &HashMap<String, TlsDomainConfig>) -> GenericResult<TlsDomains> {
        Ok(TlsDomains {
            server_name: server_name.to_owned(),
            default_domain: TlsDomain::new(&default_domain.cert, &default_domain.key)?,
            additional_domains: additional_domains.iter()
                .sorted_by_key(|(name, _)| {
                    name.chars().filter(|&c| c == '.').count()
                })
                .rev()
                .map(|(name, config)| -> GenericResult<(String, TlsDomain)> {
                    if name.starts_with('.') || name.ends_with('.') || name.contains("..") {
                        return Err!("Invalid domain name: {:?}", name);
                    }
                    dbg!(name);
                    Ok((name.to_owned(), TlsDomain::new(&config.cert, &config.key)?))
                }).collect::<Result<Vec<_>, _>>()?,
        })
    }

    pub async fn get_acceptor(&self, name: &str) -> TlsAcceptor {
        for (template, domain) in &self.additional_domains {
            if match_domains(template, name) {
                return domain.get_acceptor(&self.server_name).await;
            }
        }
        self.default_domain.get_acceptor(&self.server_name).await
    }
}

struct TlsDomain {
    cert_path: PathBuf,
    key_path: PathBuf,
    acceptor: TlsAcceptor,
}

impl TlsDomain {
    fn new(cert_path: &Path, key_path: &Path) -> GenericResult<TlsDomain> {
        Ok(TlsDomain {
            cert_path: cert_path.to_owned(),
            key_path: key_path.to_owned(),
            acceptor: load_certs(cert_path, key_path)?,
        })
    }

    async fn get_acceptor(&self, server_name: &str) -> TlsAcceptor {
        // FIXME(konishchev): Cache it + async
        load_certs(&self.cert_path, &self.key_path).unwrap_or_else(|e| {
            error!("[{}] {}.", server_name, e);
            self.acceptor.clone()
        })
    }
}

fn match_domains(template: &str, domain: &str) -> bool {
    if domain == template {
        return true
    }

    if domain.ends_with(template) {
        let pos = domain.len() - template.len();
        if domain.get(pos-1..pos) == Some(".") {
            return true;
        }
    }

    return false
}

fn load_certs(cert_path: &Path, key_path: &Path) -> GenericResult<TlsAcceptor> {
    let certs = load_cert(cert_path).map_err(|e| format!(
        "Failed to load certificate {:?}: {}", cert_path, e))?;

    let key = load_key(key_path).map_err(|e| format!(
        "Failed to load private key {:?}: {}", key_path, e))?;

    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| format!("Failed to load private key: {}", e))?;

    Ok(TlsAcceptor::from(Arc::new(config)))
}

fn load_cert(path: &Path) -> GenericResult<Vec<CertificateDer<'static>>> {
    let mut file = BufReader::new(File::open(path)?);

    let certs = rustls_pemfile::certs(&mut file).collect::<Result<Vec<_>, _>>()?;
    if certs.is_empty() {
        return Err!("the file doesn't contain any certificate");
    }

    Ok(certs)
}

fn load_key(path: &Path) -> GenericResult<PrivateKeyDer<'static>> {
    let mut file = BufReader::new(File::open(path)?);

    let mut key = Option::None;
    for item in rustls_pemfile::pkcs8_private_keys(&mut file) {
        if key.replace(item?).is_some() {
            return Err!("the file contains more than one private key")
        }
    }

    Ok(key.ok_or("the file doesn't contain any private key")?.into())
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    #[rstest(template, domain, result,
        case("some.domain", "some.domain", true),
        case("some.domain", "other.domain", false),
        case("some.domain", "sub.some.domain", true),
        case("some.domain", "sub.sub.some.domain", true),
    )]
    fn match_domains(template: &str, domain: &str, result: bool) {
        assert_eq!(super::match_domains(template, domain), result)
    }
}
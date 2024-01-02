use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    Router,
    extract::{State, Request},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::IntoResponse, response::Response,
    routing::get,
};
use log::{trace, info, error};
use prometheus_client::{
    encoding::DescriptorEncoder,
    collector::Collector,
    registry::Registry
};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

use crate::core::GenericResult;

#[derive(Debug)]
pub struct ArcCollector<T: Collector>(Arc<T>);

impl<T: Collector> ArcCollector<T> {
    pub fn new(collector: Arc<T>) -> Box<ArcCollector<T>> {
        Box::new(ArcCollector(collector))
    }
}

impl<T: Collector> Collector for ArcCollector<T> {
    fn encode(&self, encoder: DescriptorEncoder) -> Result<(), std::fmt::Error> {
        self.0.encode(encoder)
    }
}

pub async fn run(bind_address: SocketAddr, registry: Registry) -> GenericResult<JoinHandle<std::io::Result<()>>> {
    let listener = TcpListener::bind(bind_address).await.map_err(|e| format!(
        "Unable to bind to {bind_address}: {e}"))?;

    let app = Router::new()
        .route("/metrics", get(get_metrics))
        .layer(middleware::from_fn(logging_middleware))
        .with_state(Arc::new(registry));

    info!("[Metrics] Listening on {bind_address}.");
    Ok(tokio::spawn(async move {
        axum::serve(listener, app).await
    }))
}

async fn get_metrics(State(registry): State<Arc<Registry>>) -> impl IntoResponse {
    let mut result = String::new();

    match prometheus_client::encoding::text::encode(&mut result, &registry) {
        Ok(()) => {
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/openmetrics-text; version=1.0.0; charset=utf-8")
                .body(result)
                .unwrap()
        },
        Err(err) => {
            error!("[Metrics] Failed to collect metrics: {err}.");
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .header(header::CONTENT_TYPE, "text/plain")
                .body("Metrics collection error".to_owned())
                .unwrap()
        },
    }
}

async fn logging_middleware(request: Request, next: Next) -> Response {
    trace!("[Metrics] {} {}", request.method(), request.uri());
    next.run(request).await
}
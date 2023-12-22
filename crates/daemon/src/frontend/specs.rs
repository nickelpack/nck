use std::{
    ops::Deref,
    sync::{Arc, LazyLock},
};

use anyhow::Context;
use axum::{
    extract::{Path, State},
    routing::post,
    Router,
};
use axum_core::{body::Body, extract::Request, response::Response};
use dashmap::DashMap;
use futures::StreamExt;
use hyper::{header, HeaderMap, StatusCode};
use nck_core::hashing::SupportedHash;
use rand::Rng;
use tokio::{io::AsyncWriteExt, sync::Mutex};

use crate::{
    spec::{OutputName, PackageName},
    store::StoreLock,
    string_types::{self, Base32, Error, Hash, UrlValue},
};

use super::FrontendState;

static SRC_PACKAGE_NAME: LazyLock<PackageName> = LazyLock::new(|| "src".parse().unwrap());
static DEFAULT_OUTPUT: LazyLock<OutputName> = LazyLock::new(|| "out".parse().unwrap());

type Result<R = Response, E = Error> = string_types::Result<R, E>;

#[derive(Debug)]
struct PendingSpec {
    name: PackageName,
    locks: Vec<StoreLock>,
}

impl PendingSpec {
    pub fn new(name: PackageName) -> Self {
        PendingSpec {
            name,
            locks: Vec::new(),
        }
    }
}

#[derive(Debug)]
struct InnerState {
    pending_specs: DashMap<PackageName, Arc<Mutex<PendingSpec>>>,
    frontend_state: FrontendState,
}

#[derive(Debug, Clone)]
struct SpecsState(Arc<InnerState>);

impl Deref for SpecsState {
    type Target = InnerState;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub fn create_routes(frontend_state: FrontendState) -> Router {
    Router::new()
        .route("/:name", post(create_spec))
        .route("/:name/upload", post(upload_file))
        .with_state(SpecsState(Arc::new(InnerState {
            pending_specs: DashMap::new(),
            frontend_state,
        })))
}

async fn create_spec(
    State(state): State<SpecsState>,
    Path(name): Path<UrlValue<PackageName>>,
) -> Result {
    let name = name.into_inner();

    let mut bytes = [0u8; 16];
    rand::thread_rng().try_fill(&mut bytes)?;

    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_micros();

    let mut hash = blake3::Hasher::new();
    hash.update(&bytes);
    hash.update(&time.to_ne_bytes());

    let base32 = Base32::from(*hash.finalize().as_bytes());
    let id = format!("{name}-{base32}");

    let builder = PendingSpec::new(name);

    state
        .pending_specs
        .insert(id.parse().unwrap(), Arc::new(Mutex::new(builder)));

    let response = Response::builder()
        .status(StatusCode::CREATED)
        .header(header::LOCATION, format!("/api/1/spec/{id}"))
        .body(Body::empty())?;
    Ok(response)
}

async fn upload_file(
    State(state): State<SpecsState>,
    Path(spec): Path<UrlValue<PackageName>>,
    header_map: HeaderMap,
    body: Request,
) -> Result {
    let spec = spec.into_inner();
    let mut spec = if let Some(spec) = state.pending_specs.get(&spec) {
        spec.clone().lock_owned().await
    } else {
        Error::not_found("spec not found").err()?;
    };

    if let Some(existing_hash) = header_map.get("If-None-Match") {
        let v = existing_hash.as_bytes();
        let v = if v.starts_with(b"\"") && v.ends_with(b"\"") {
            &v[1..(v.len() - 2)]
        } else {
            Error::bad_request(()).err()?
        };

        let hash: Hash = v
            .try_into()
            .with_context(|| "invalid If-None-Match value")?;

        let file = state.frontend_state.store.get_file(hash.inner()).await?;
        let id = file
            .as_path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        spec.locks.push(file);

        let response = Response::builder()
            .status(StatusCode::SEE_OTHER)
            .header(header::ETAG, format!("\"{hash}\""))
            .header(header::LOCATION, format!("/api/1/download/{id}"))
            .body(Body::empty())?;
        return Ok(response);
    }

    let mut file = state.frontend_state.store.create_file().await?;
    let mut body = body.into_body().into_data_stream();
    let mut hash = blake3::Hasher::new();

    while let Some(val) = body.next().await {
        let mut val = val?;
        hash.update(&val[..]);
        file.write_all_buf(&mut val)
            .await
            .with_context(|| "while writing to temporary upload file")?;
    }

    let hash = SupportedHash::Blake3(*hash.finalize().as_bytes());
    let final_lock = file.complete(&hash).await?;
    let id = final_lock
        .as_path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    spec.locks.push(final_lock);

    let hash = Hash::new(hash);
    let response = Response::builder()
        .status(StatusCode::CREATED)
        .header(header::ETAG, format!("\"{hash}\""))
        .header(header::LOCATION, format!("/api/1/download/{id}"))
        .body(Body::empty())?;

    Ok(response)
}

use std::{collections::HashSet, io::ErrorKind, sync::Arc};

use axum::{
    extract::{Path, State},
    routing::post,
    Json, Router,
};
use axum_core::{body::Body, extract::Request, response::Response};
use dashmap::{mapref::entry::Entry, DashMap};
use derive_more::{Deref, DerefMut};
use futures::StreamExt;
use hyper::{header, HeaderMap, StatusCode};
use nck_hashing::{SupportedHash, SupportedHasher};
use nck_spec::Spec;
use tokio::{
    io::AsyncWriteExt,
    sync::{Mutex, OwnedMappedMutexGuard, OwnedMutexGuard},
};

use crate::{
    app_error,
    axum_extensions::{AppError, AppErrorOption, AppErrorReason},
    store::StoreLock,
};

use super::FrontendState;

#[derive(Debug, Default)]
struct PendingSpecState {
    locks: HashSet<StoreLock>,
}

#[derive(Debug, Default)]
struct PendingSpec(Arc<Mutex<Option<PendingSpecState>>>);

impl PendingSpec {
    async fn lock(
        &self,
    ) -> Option<OwnedMappedMutexGuard<Option<PendingSpecState>, PendingSpecState>> {
        let guard = self.0.clone().lock_owned().await;
        if guard.is_some() {
            let unwrapped = OwnedMutexGuard::map(guard, |v| v.as_mut().unwrap());
            Some(unwrapped)
        } else {
            None
        }
    }

    async fn take(&self) -> Option<PendingSpecState> {
        let mut guard = self.0.clone().lock_owned().await;
        guard.take()
    }
}

#[derive(Debug, Deref, DerefMut)]
struct InnerState {
    pending_specs: DashMap<String, PendingSpec>,
    #[deref]
    #[deref_mut]
    frontend_state: FrontendState,
}

#[derive(Debug, Clone, Deref, DerefMut)]
struct SpecsState(Arc<InnerState>);

pub fn create_routes(frontend_state: FrontendState) -> Router {
    Router::new()
        .route("/", post(create_spec))
        .route("/:name/add_file", post(add_file))
        .route("/:name/run", post(run))
        .with_state(SpecsState(Arc::new(InnerState {
            pending_specs: DashMap::new(),
            frontend_state,
        })))
}

async fn create_spec(State(state): State<SpecsState>) -> Result<Response, AppError> {
    let name = loop {
        let pet = petname::petname(3, "-");
        if let Entry::Vacant(vacant) = state.pending_specs.entry(pet) {
            break vacant.key().clone();
        }
    };

    tracing::debug!(?name, "pending spec created");

    let response = Response::builder()
        .status(StatusCode::CREATED)
        .header(header::LOCATION, format!("/api/1/spec/{name}"))
        .body(Body::empty())
        .reason("creating response")?;
    Ok(response)
}

async fn add_file(
    State(state): State<SpecsState>,
    Path(spec_name): Path<String>,
    header_map: HeaderMap,
    body: Request,
) -> Result<Response, AppError> {
    // TODO: optionally accept multipart here.

    let mut spec = state
        .pending_specs
        .get(&spec_name)
        .ok_or_else_message(|| format!("spec {} not found", spec_name))?
        .lock()
        .await;

    let spec = spec.as_mut().ok_or_else_message(|| {
        format!("spec {} has already been submitted for build", spec_name)
    })?;

    if let Some(existing_hash) = header_map.get("If-None-Match") {
        let v = existing_hash.as_bytes();
        let v = if v.starts_with(b"\"") && v.ends_with(b"\"") {
            &v[1..(v.len() - 2)]
        } else {
            app_error!("parsing If-None-Match value")
                .err()
                .with_message(|| "invalid If-None-Match value".to_string())
                .status_code(StatusCode::BAD_REQUEST)?;
        };

        let hash = std::str::from_utf8(v)
            .reason("parsing If-None-Match value")
            .with_message(|| "invalid If-None-Match value".to_string())
            .status_code(StatusCode::BAD_REQUEST)?;

        let hash: SupportedHash = hash.parse().reason("test")?;

        match state.frontend_state.store.get_file(&hash).await {
            Ok(file) => {
                tracing::debug!("file already cached");
                spec.locks.insert(file);

                let response = Response::builder()
                    .status(StatusCode::SEE_OTHER)
                    .header(header::ETAG, format!("\"{hash}\""))
                    .header(header::LOCATION, format!("/api/1/download/{hash}"))
                    .body(Body::empty())
                    .reason("building response")?;
                return Ok(response);
            }
            Err(e) if e.kind() == ErrorKind::NotFound => {
                tracing::trace!("file not cached");
            }
            Err(other) => Err(other).reason("querying the store")?,
        }
    }

    let mut file = state
        .frontend_state
        .store
        .create_file()
        .await
        .reason("creating a temporary file to upload into")?;
    let mut body = body.into_body().into_data_stream();
    let mut hash = SupportedHasher::blake3();

    tracing::debug!("accepting uploaded data");

    while let Some(val) = body.next().await {
        let mut val = val.reason("reading request")?;
        hash.update(&val[..]);
        file.write_all_buf(&mut val)
            .await
            .reason("writing to temporary upload file")?;
    }

    let hash = hash.finalize();
    let final_lock = file
        .complete(&hash)
        .await
        .reason("committing the file to the store")?;
    spec.locks.insert(final_lock);

    tracing::debug!(%hash, "file uploaded");

    let response = Response::builder()
        .status(StatusCode::CREATED)
        .header(header::ETAG, format!("\"{hash}\""))
        .header(header::LOCATION, format!("/api/1/download/{hash}"))
        .body(Body::empty())
        .reason("creating response")?;

    Ok(response)
}

async fn run(
    State(state): State<SpecsState>,
    Path(spec): Path<String>,
    Json(body): Json<Spec>,
) -> Result<Response, AppError> {
    let pending = state
        .pending_specs
        .remove(&spec)
        .ok_or_else_message(|| format!("spec {} not found", spec))?
        .1
        .take()
        .await
        .ok_or_else_message(|| format!("spec {} has already been submitted", spec))?;

    let locks: Vec<_> = pending.locks.into_iter().collect();

    state
        .0
        .frontend_state
        .store
        .clone()
        .start(body, locks)
        .await
        .reason("starting the build")?;

    let response = Response::builder()
        .status(StatusCode::ACCEPTED)
        .body(Body::empty())
        .reason("creating response")?;
    Ok(response)
}

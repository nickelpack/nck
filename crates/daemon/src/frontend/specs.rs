use std::{
    collections::{BTreeMap, HashSet},
    ffi::OsString,
    io::ErrorKind,
    ops::Deref,
    os::unix::prelude::OsStringExt,
    path::PathBuf,
    str::FromStr,
    sync::Arc,
};

use anyhow::Context;
use axum::{
    extract::{Multipart, Path, State},
    routing::post,
    Router,
};
use axum_core::{body::Body, extract::Request, response::Response};
use axum_extra::extract::Query;
use dashmap::DashMap;
use futures::StreamExt;
use hyper::{header, HeaderMap, StatusCode};
use nck_hashing::SupportedHash;
use rand::Rng;
use serde::Deserialize;
use tokio::{io::AsyncWriteExt, sync::Mutex};

use crate::{
    spec::{CompressionAlgorithm, OutputName, PackageName, Spec},
    store::StoreLock,
    string_types::{self, Error, UrlValue},
};

use super::FrontendState;

type Result<R = Response, E = Error> = string_types::Result<R, E>;

#[derive(Debug)]
struct PendingSpec {
    spec: Spec,
    locks: HashSet<StoreLock>,
}

impl PendingSpec {
    pub fn new(name: PackageName) -> Self {
        PendingSpec {
            spec: Spec::new(name),
            locks: HashSet::new(),
        }
    }
}

#[derive(Debug)]
struct InnerState {
    pending_specs: DashMap<PackageName, Arc<Mutex<Option<PendingSpec>>>>,
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
        .route("/:name/action/extract", post(extract_file))
        .route("/:name/action/execute", post(execute))
        .route("/:name/run", post(run))
        .with_state(SpecsState(Arc::new(InnerState {
            pending_specs: DashMap::new(),
            frontend_state,
        })))
}

#[derive(Debug, Deserialize)]
struct CreateSpec {
    output: Vec<String>,
}

async fn create_spec(
    State(state): State<SpecsState>,
    Path(name): Path<UrlValue<PackageName>>,
    Query(CreateSpec { output }): Query<CreateSpec>,
) -> Result {
    let name = name.into_inner();

    let mut bytes = [0u8; 16];
    rand::thread_rng().try_fill(&mut bytes)?;

    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_micros();

    let pet = petname::petname(3, "-");
    let id = format!("{name}-{pet}");

    let mut builder = PendingSpec::new(name);
    for output in output {
        builder.spec.add_output(output.parse::<OutputName>()?);
    }

    state
        .pending_specs
        .insert(id.parse().unwrap(), Arc::new(Mutex::new(Some(builder))));

    tracing::debug!(?id, "pending spec created");

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
    // TODO: Use multipart here for consistency.

    let spec = spec.into_inner();
    let mut spec = state
        .pending_specs
        .get(&spec)
        .ok_or_else(|| Error::not_found("spec not found"))?
        .clone()
        .lock_owned()
        .await;
    let spec = spec
        .as_mut()
        .ok_or_else(|| Error::not_found("spec already executing"))?;

    if let Some(existing_hash) = header_map.get("If-None-Match") {
        let v = existing_hash.as_bytes();
        let v = if v.starts_with(b"\"") && v.ends_with(b"\"") {
            &v[1..(v.len() - 2)]
        } else {
            Error::bad_request(()).err()?
        };

        let hash: SupportedHash = std::str::from_utf8(v)
            .with_context(|| "invalid If-None-Match value")?
            .parse()
            .with_context(|| "invalid If-None-Match value")?;

        match state.frontend_state.store.get_file(&hash).await {
            Ok(file) => {
                tracing::debug!("file already cached");
                spec.locks.insert(file);

                let response = Response::builder()
                    .status(StatusCode::SEE_OTHER)
                    .header(header::ETAG, format!("\"{hash}\""))
                    .header(header::LOCATION, format!("/api/1/download/{hash}"))
                    .body(Body::empty())?;
                return Ok(response);
            }
            Err(e) if e.kind() == ErrorKind::NotFound => {
                tracing::trace!("file not cached");
            }
            Err(other) => Err(other)?,
        }
    }

    let mut file = state.frontend_state.store.create_file().await?;
    let mut body = body.into_body().into_data_stream();
    let mut hash = blake3::Hasher::new();

    tracing::debug!("accepting uploaded data");

    while let Some(val) = body.next().await {
        let mut val = val?;
        hash.update(&val[..]);
        file.write_all_buf(&mut val)
            .await
            .with_context(|| "while writing to temporary upload file")?;
    }

    let hash = SupportedHash::Blake3(*hash.finalize().as_bytes());
    let final_lock = file.complete(&hash).await?;
    spec.locks.insert(final_lock);

    tracing::debug!(%hash, "file uploaded");

    let response = Response::builder()
        .status(StatusCode::CREATED)
        .header(header::ETAG, format!("\"{hash}\""))
        .header(header::LOCATION, format!("/api/1/download/{hash}"))
        .body(Body::empty())?;

    Ok(response)
}

async fn extract_file(
    State(state): State<SpecsState>,
    Path(spec): Path<UrlValue<PackageName>>,
    mut multipart: Multipart,
) -> Result {
    let spec = spec.into_inner();
    let mut spec = state
        .pending_specs
        .get(&spec)
        .ok_or_else(|| Error::not_found("spec not found"))?
        .clone()
        .lock_owned()
        .await;
    let spec = spec
        .as_mut()
        .ok_or_else(|| Error::not_found("spec already executing"))?;

    let mut source = None;
    let mut dest = None;
    let mut compression = None;
    while let Some(field) = multipart.next_field().await? {
        match field.name() {
            Some("source") => {
                let text = field.text().await?;
                let old = source.replace(SupportedHash::from_str(text.as_str())?);
                if old.is_some() {
                    Error::bad_request("duplicate source field").err()?;
                }
            }
            Some("dest") => {
                let bytes = field.bytes().await?.to_vec();
                let old = dest.replace(PathBuf::from(OsString::from_vec(bytes)));
                if old.is_some() {
                    Error::bad_request("duplicate dest field").err()?;
                }
            }
            Some("compression") => {
                let text = field.text().await?;
                let alg = CompressionAlgorithm::from_str(text.as_str())?;
                let old = compression.replace(alg);
                if old.is_some() {
                    Error::bad_request("duplicate compression field").err()?;
                }
            }
            Some(other) => Error::bad_request(format!("unknown field {}", other)).err()?,
            None => Error::bad_request("all fields must have a name").err()?,
        }
    }

    let source = source.ok_or_else(|| Error::bad_request("source field is required"))?;
    let dest = dest.ok_or_else(|| Error::bad_request("dest field is required"))?;
    let compression = compression.unwrap_or_default();

    match state.0.frontend_state.store.get_file(&source).await {
        Ok(lock) => {
            spec.locks.insert(lock);
        }
        Err(e) if e.kind() == ErrorKind::NotFound => Error::not_found("source not found").err()?,
        Err(e) => Err(e)?,
    }

    spec.spec.push_action(crate::spec::Action::Extract(
        source,
        dest.clone(),
        compression,
    ));

    tracing::debug!(%source, ?dest, "extract action added");

    let response = Response::builder()
        .status(StatusCode::ACCEPTED)
        .body(Body::empty())?;
    Ok(response)
}

async fn execute(
    State(state): State<SpecsState>,
    Path(spec): Path<UrlValue<PackageName>>,
    mut multipart: Multipart,
) -> Result {
    let spec = spec.into_inner();
    let mut spec = state
        .pending_specs
        .get(&spec)
        .ok_or_else(|| Error::not_found("spec not found"))?
        .clone()
        .lock_owned()
        .await;
    let spec = spec
        .as_mut()
        .ok_or_else(|| Error::not_found("spec already executing"))?;

    let mut bin = None;
    let mut args = BTreeMap::<usize, OsString>::new();
    let mut env = BTreeMap::<OsString, OsString>::new();
    while let Some(field) = multipart.next_field().await? {
        match field.name() {
            Some("bin") => {
                let bytes = field.bytes().await?.to_vec();
                let old = bin.replace(OsString::from_vec(bytes));
                if old.is_some() {
                    Error::bad_request("duplicate bin field").err()?;
                }
            }
            Some(other) => {
                if let Some(index) = parse_array::<usize>("arg", other) {
                    if args.contains_key(&index) {
                        Error::bad_request(format!("duplicate {} field", other)).err()?
                    }
                    let bytes = field.bytes().await?.to_vec();
                    args.insert(index, OsString::from_vec(bytes));
                } else if let Some(name) = parse_array::<OsString>("env", other) {
                    if env.contains_key(&name) {
                        Error::bad_request(format!("duplicate {} field", other)).err()?
                    }
                    let bytes = field.bytes().await?.to_vec();
                    env.insert(name, OsString::from_vec(bytes));
                } else {
                    Error::bad_request(format!("unknown field {}", other)).err()?
                }
            }
            None => Error::bad_request("all fields must have a name").err()?,
        }
    }

    let bin = bin.ok_or_else(|| Error::bad_request("bin field is missing"))?;

    let args: Vec<_> = args.into_values().collect();
    spec.spec
        .push_action(crate::spec::Action::Execute(PathBuf::from(&bin), args, env));

    tracing::debug!(?bin, "execute");

    let response = Response::builder()
        .status(StatusCode::ACCEPTED)
        .body(Body::empty())?;
    Ok(response)
}

async fn run(State(state): State<SpecsState>, Path(spec): Path<UrlValue<PackageName>>) -> Result {
    let spec = spec.into_inner();
    let mut spec = state
        .pending_specs
        .remove(&spec)
        .ok_or_else(|| Error::not_found("spec not found"))?
        .1
        .clone()
        .lock_owned()
        .await;
    let spec = spec
        .take()
        .ok_or_else(|| Error::not_found("spec already executing"))?;

    if !spec
        .spec
        .actions_iter()
        .any(|f| matches!(f, &crate::spec::Action::Execute(_, _, _)))
    {
        Error::bad_request("at least one execute action is required").err()?;
    }

    let locks: Vec<_> = spec.locks.into_iter().collect();
    let spec = spec.spec;

    state
        .0
        .frontend_state
        .store
        .clone()
        .start(spec, locks)
        .await?;

    let response = Response::builder()
        .status(StatusCode::ACCEPTED)
        .body(Body::empty())?;
    Ok(response)
}

fn parse_array<T: FromStr>(name: &str, v: &str) -> Option<T> {
    if !v.starts_with(name) {
        return None;
    }
    let v = &v[name.len()..];

    if !v.starts_with('[') || !v.ends_with(']') {
        return None;
    }
    let v = &v[1..(v.len() - 1)];

    T::from_str(v).ok()
}

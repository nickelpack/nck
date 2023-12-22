use std::{
    ffi::OsString,
    io::ErrorKind,
    sync::{Arc, LazyLock},
};

use axum::extract::{Path, Query, State};
use axum_core::{body::Body, extract::Request, response::Response};
use color_eyre::eyre::Context;
use futures::StreamExt;
use hyper::{header, HeaderMap, StatusCode};
use nck_core::{
    hashing::SupportedHash,
    io::TempFile,
    spec::{OutputName, PackageName},
};
use rand::Rng;
use serde::Deserialize;
use tokio::io::AsyncWriteExt;

use crate::string_types::Base32;

use self::url_types::{Error, Hash, Result, UrlValue};

pub mod url_types;

static SRC_PACKAGE_NAME: LazyLock<PackageName> = LazyLock::new(|| "src".parse().unwrap());
static DEFAULT_OUTPUT: LazyLock<OutputName> = LazyLock::new(|| "out".parse().unwrap());

pub async fn create_spec(
    State(state): State<AppState>,
    Path(name): Path<UrlValue<PackageName>>,
) -> Result<Response> {
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

    let builder = SpecBuilder::new(name);
    builder.outputs.insert(DEFAULT_OUTPUT.clone());

    state.formulas.insert(id.clone(), Arc::new(builder));

    let response = Response::builder()
        .status(StatusCode::CREATED)
        .header(header::LOCATION, format!("/api/1/formulas/{id}"))
        .body(Body::empty())?;
    Ok(response)
}

pub async fn formula_write_file(
    State(state): State<AppState>,
    Path(id): Path<String>,
    header_map: HeaderMap,
    body: Request,
) -> Result {
    let formula = state
        .formulas
        .get(&id)
        .ok_or_else(|| Error::not_found(()))?
        .clone();

    if let Some(existing_hash) = header_map.get("If-None-Match") {
        let v = existing_hash.as_bytes();
        let v = if v.starts_with(b"\"") && v.ends_with(b"\"") {
            &v[1..(v.len() - 2)]
        } else {
            Error::bad_request(()).err()?
        };

        let hash: Hash = v
            .try_into()
            .wrap_err_with(|| "invalid If-None-Match value")?;
        let package = PackageReference {
            name: SRC_PACKAGE_NAME.clone(),
            hash: *hash.inner(),
            output: DEFAULT_OUTPUT.clone(),
        };

        let id = package.local_path();
        let path = package.path();

        if tokio::fs::try_exists(path.as_path())
            .await
            .wrap_err_with(|| "while attempting to check if the file already exists")?
        {
            formula.locked.insert(path.clone());
            let response = Response::builder()
                .status(StatusCode::SEE_OTHER)
                .header(header::ETAG, format!("\"{hash}\""))
                .header(header::LOCATION, format!("/api/1/download/{id}"))
                .body(Body::empty())?;
            return Ok(response);
        } else {
            tracing::warn!(?path, "the broken link is being replaced");
            tokio::fs::remove_file(path.as_path())
                .await
                .wrap_err_with(|| "when removing broken link")?;
        }
    }

    let (tmp, mut f) = TempFile::new_in(TMP_DIRECTORY.as_path())
        .await
        .wrap_err_with(|| "allocating a temporary file for uploading into")?;

    let mut body = body.into_body().into_data_stream();
    let mut hash = blake3::Hasher::new();

    while let Some(val) = body.next().await {
        let mut val = val?;
        hash.update(&val[..]);
        f.write_all_buf(&mut val)
            .await
            .wrap_err_with(|| "while writing to temporary upload file")?;
    }
    drop(f);

    let hash = Hash::new(SupportedHash::Blake3(*hash.finalize().as_bytes()));
    let path = PackageReference {
        name: SRC_PACKAGE_NAME.clone(),
        hash: *hash.inner(),
        output: DEFAULT_OUTPUT.clone(),
    }
    .path();

    match tokio::fs::rename(tmp.as_path(), path.as_path()).await {
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
        other => other?,
    }

    tmp.forget();
    formula.locked.insert(path.clone());

    let response = Response::builder()
        .status(StatusCode::CREATED)
        .header(header::ETAG, format!("\"{hash}\""))
        .header(header::LOCATION, format!("/api/1/download/{id}"))
        .body(Body::empty())?;

    Ok(response)
}

#[derive(Debug, Deserialize)]
pub struct CopyFormulaFile {
    pub to: String,
    pub executable: Option<()>,
}

pub async fn formula_copy_file(
    State(state): State<AppState>,
    Path((id, from)): Path<(String, Hash)>,
    Query(query): Query<CopyFormulaFile>,
) -> Result {
    let formula = state
        .formulas
        .get(&id)
        .ok_or_else(|| Error::not_found(()))?
        .clone();

    let package = PackageReference {
        name: SRC_PACKAGE_NAME.clone(),
        hash: *from.inner(),
        output: DEFAULT_OUTPUT.clone(),
    };
    let source = package.path();

    if !tokio::fs::try_exists(source.as_path())
        .await
        .wrap_err_with(|| "while determining if the source file exists")?
    {
        Error::not_found(()).err()?;
    }

    formula.locked.insert(source.clone());
    formula.copy.insert(
        query.to.into(),
        crate::spec_builder::CopiedFile {
            package,
            executable: query.executable.is_some(),
        },
    );

    let response = Response::builder()
        .status(StatusCode::OK)
        .body(Body::empty())?;

    Ok(response)
}

pub async fn formula_set_env(
    State(state): State<AppState>,
    Path((id, name)): Path<(String, String)>,
    body: String,
) -> Result {
    let formula = state
        .formulas
        .get(&id)
        .ok_or_else(|| Error::not_found(()))?
        .clone();

    let name = OsString::from(name);
    let value = OsString::from(body);

    formula.env.insert(name, value);

    let response = Response::builder()
        .status(StatusCode::OK)
        .body(Body::empty())?;

    Ok(response)
}

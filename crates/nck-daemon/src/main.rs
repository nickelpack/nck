#![feature(result_option_inspect)]
#![feature(async_closure)]

use std::{fmt::Display, io::ErrorKind, path::PathBuf, process::ExitCode, sync::Arc};

use axum::{
    extract::{Path, Request, State},
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
    Router,
};
use blake3::Hasher;
use config::Environment;
use dashmap::DashMap;
use futures_util::StreamExt;
use nck_sandbox::current::{Controller, Sandbox};
use nck_util::{base32, io::TempFile};
use rand::RngCore;
use serde::de::Visitor;
use tokio::{io::AsyncWriteExt, sync::Mutex};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Clone, serde::Deserialize)]
struct StoreConfig {
    #[serde(default = "default_store_directory")]
    directory: PathBuf,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct Config {
    #[serde(default = "default_store_config")]
    store: StoreConfig,
    sandbox: nck_sandbox::Config,
}

fn default_store_directory() -> PathBuf {
    "/var/nck/store".into()
}

fn default_store_config() -> StoreConfig {
    StoreConfig {
        directory: default_store_directory(),
    }
}

fn main() -> ExitCode {
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("failed to set subscriber");

    let config = config::Config::builder()
        .add_source(Environment::with_prefix("nck").separator("__"))
        .build()
        .unwrap();
    let config: Config = config.try_deserialize().unwrap();

    let result = nck_sandbox::current::main(config.sandbox, |c| controller_main(c, config.store));
    match result {
        Some(Err(error)) => {
            tracing::error!(?error, "controller failed");
            ExitCode::FAILURE
        }
        _ => ExitCode::SUCCESS,
    }
}

struct AppState {
    store_config: StoreConfig,
    controller: Controller,
    sandboxes: DashMap<String, Sandbox>,
    formulas: DashMap<String, Arc<Mutex<Formula>>>,
}

struct Formula {
    prefix: String,
    actions: Vec<Action>,
}

impl Default for Formula {
    fn default() -> Self {
        Self {
            prefix: String::default(),
            actions: Vec::new(),
        }
    }
}

impl Formula {
    pub fn stable_hash(&self, h: &mut Hasher) {
        for act in self.actions.iter() {
            act.stable_hash(h);
        }
    }
}

enum Action {
    UploadFile(BlakeId),
}

impl Action {
    pub fn stable_hash(&self, h: &mut Hasher) {
        match self {
            Action::UploadFile(file_hash) => {
                h.update(&1u32.to_be_bytes());
                h.update(&file_hash.data);
            }
        }
    }
}

#[tracing::instrument(level = "trace", name = "main", skip_all)]
async fn controller_main(controller: Controller, store: StoreConfig) -> anyhow::Result<()> {
    let state = Arc::new(AppState {
        controller,
        sandboxes: DashMap::new(),
        formulas: DashMap::new(),
        store_config: store,
    });

    // TODO: This should be a unix socket by default (network optional), but it requires quite a bit of code.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3100")
        .await
        .unwrap();
    println!("listening on {}", listener.local_addr().unwrap());

    let app = Router::new()
        .route("/api/1_1/formulas/:suffix", post(create_formula))
        .route(
            "/api/1_1/formulas/:formula/write/:hash",
            post(formula_upload_file),
        )
        .route("/api/1_1/formulas/:formula/build", post(formula_build))
        .with_state(state);

    axum::serve(listener, app).await.unwrap();
    Ok(())
}

async fn create_formula(
    State(state): State<Arc<AppState>>,
    Path(mut prefix): Path<String>,
) -> impl IntoResponse {
    let mut rand_buffer = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut rand_buffer);

    let formula = Formula {
        prefix: prefix.clone(),
        ..Default::default()
    };

    let id = blake3::Hasher::new()
        .update(
            &std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
                .to_be_bytes(),
        )
        .update(&rand_buffer)
        .finalize();
    prefix.push('-');
    nck_util::base32::encode_into(id.as_bytes(), &mut prefix);

    let mut headers = HeaderMap::new();
    headers.insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    headers.insert(
        header::LOCATION,
        format!("/api/1_1/formulas/{prefix}").parse().unwrap(),
        // TODO: Indicate expiry
    );

    state.formulas.insert(prefix, Arc::new(Mutex::new(formula)));
    (StatusCode::CREATED, headers)
}

async fn formula_upload_file(
    State(state): State<Arc<AppState>>,
    Path((formula, mut hash)): Path<(String, BlakeId)>,
    request: Request,
) -> impl IntoResponse {
    let mut headers = HeaderMap::new();
    headers.insert(header::CACHE_CONTROL, "no-store".parse().unwrap());

    let mut path = state.store_config.directory.join(hash.to_string());

    let formula_data = match state.formulas.get(&formula) {
        Some(item) => item,
        None => return (StatusCode::NOT_FOUND, headers),
    };

    let (temp_file, mut file) = match TempFile::new().await {
        Ok(f) => f,
        Err(error) => {
            tracing::error!(?error, "failed to create temporary file for upload");
            return (StatusCode::INTERNAL_SERVER_ERROR, headers);
        }
    };

    if hash != BlakeId::default() {
        match tokio::fs::try_exists(path.as_path()).await {
            Ok(true) => {
                let mut formula = formula_data.lock().await;
                formula.actions.push(Action::UploadFile(hash));
                return (StatusCode::CONFLICT, headers);
            }
            Ok(false) => {}
            Err(error) => {
                tracing::warn!(?error, ?path, "error determining if file exists in store");
                match tokio::fs::remove_file(path.as_path()).await {
                    Err(e) if e.kind() == ErrorKind::NotFound => {}
                    Err(error) => {
                        tracing::error!(
                            ?error,
                            path  = ?path.as_path(),
                            "failed to move the broken file out of the way"
                        );
                        return (StatusCode::INTERNAL_SERVER_ERROR, headers);
                    }
                    _ => {}
                }
            }
        };
    }

    let mut stream = request.into_body().into_data_stream();
    let mut actual_hash = blake3::Hasher::new();
    while let Some(data) = stream.next().await {
        let data = match data {
            Ok(data) => data,
            Err(error) => {
                tracing::info!(?error, "failure during upload");
                return (StatusCode::INTERNAL_SERVER_ERROR, headers);
            }
        };

        if let Ok(true) = tokio::fs::try_exists(path.as_path()).await {
            let mut formula = formula_data.lock().await;
            formula.actions.push(Action::UploadFile(hash));
            return (StatusCode::OK, headers);
        }

        actual_hash.update(&data);
        if let Err(error) = file.write_all(&data).await {
            tracing::info!(?error, "failure during upload");
            return (StatusCode::INTERNAL_SERVER_ERROR, headers);
        }
    }

    let mut status = StatusCode::OK;

    let checked_hash = actual_hash.finalize();
    if hash == BlakeId::default() || checked_hash.as_bytes() != &hash.data {
        let actual_hash = base32::encode(checked_hash.as_bytes());
        let requested_hash = hash.to_string();

        if hash != BlakeId::default() {
            tracing::info!(actual_hash, requested_hash, "upload hash did not match");
        }

        status = StatusCode::SEE_OTHER;
        hash = BlakeId {
            data: *checked_hash.as_bytes(),
        };
        path = state.store_config.directory.join(hash.to_string());

        headers.insert(
            header::LOCATION,
            format!("/api/1_1/formulas/{formula}/write/{hash}")
                .parse()
                .unwrap(),
        );
        headers.insert("X-NCK-HASH", hash.to_string().parse().unwrap());
    }

    match tokio::fs::rename(temp_file.as_path(), path.as_path()).await {
        Ok(_) => {
            temp_file.forget();
        }
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
        Err(error) => {
            tracing::info!(?error, "could not move temporary");
            return (StatusCode::INTERNAL_SERVER_ERROR, headers);
        }
    }

    let mut formula = formula_data.lock().await;
    formula.actions.push(Action::UploadFile(hash));
    (status, headers)
}

async fn formula_build(
    State(state): State<Arc<AppState>>,
    Path(formula): Path<String>,
) -> impl IntoResponse {
    let headers = HeaderMap::new();
    let formula = match state.formulas.remove(&formula) {
        Some(item) => item.1,
        None => return (StatusCode::NOT_FOUND, headers),
    };

    // This should be an Option so that this route can .take it.
    let formula = formula.lock().await;
    let mut hasher = Hasher::new();
    formula.stable_hash(&mut hasher);

    let mut id = formula.prefix.clone();
    id.push('-');
    base32::encode_into(hasher.finalize().as_bytes(), &mut id);

    let mut headers = HeaderMap::new();
    headers.insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    headers.insert(
        header::LOCATION,
        format!("/api/1_1/builds/{id}").parse().unwrap(),
        // TODO: Indicate expiry
    );

    (StatusCode::CREATED, headers)
}

#[derive(Default, PartialEq)]
struct BlakeId {
    data: [u8; 32],
}

impl Display for BlakeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&base32::encode(self.data))
    }
}

impl<'de> serde::Deserialize<'de> for BlakeId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_str(BlakeIdVisitor)
    }
}

struct BlakeIdVisitor;

impl<'de> Visitor<'de> for BlakeIdVisitor {
    type Value = BlakeId;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("a RFC4648 base32-encoded blake3 hash")
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        let mut result = BlakeId { data: [0u8; 32] };
        if v == "-" {
            return Ok(result);
        }

        base32::decode_into(v, &mut &mut result.data[..]).map_err(|_| {
            E::invalid_value(
                serde::de::Unexpected::Str(v),
                &"a RFC4648 base32-encoded blake3 hash",
            )
        })?;
        Ok(result)
    }
}

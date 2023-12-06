#![feature(result_option_inspect)]
#![feature(async_closure)]

use std::{collections::HashMap, process::ExitCode, sync::Arc};

use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
    Router,
};
use blake3::Hasher;
use config::Environment;
use dashmap::DashMap;
use nck_sandbox::current::{Controller, Sandbox};
use nck_util::base32;
use rand::RngCore;
use serde::de::Visitor;
use tokio::sync::Mutex;
use tracing_subscriber::EnvFilter;

fn main() -> ExitCode {
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("failed to set subscriber");

    let config = config::Config::builder()
        .add_source(Environment::with_prefix("nck").separator("__"))
        .build()
        .unwrap();
    let config = config.try_deserialize().unwrap();

    let result = nck_sandbox::current::main(config, controller_main);
    match result {
        Some(Err(error)) => {
            tracing::error!(?error, "controller failed");
            ExitCode::FAILURE
        }
        _ => ExitCode::SUCCESS,
    }
}

struct AppState {
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
    UploadFile(BlakeId, String),
}

impl Action {
    pub fn stable_hash(&self, h: &mut Hasher) {
        match self {
            Action::UploadFile(file_hash, file_path) => {
                h.update(&1u32.to_be_bytes());
                h.update(&file_hash.data);
                h.update(&file_path.len().to_be_bytes());
                h.update(file_path.as_bytes());
            }
        }
    }
}

#[tracing::instrument(level = "trace", name = "main", skip_all)]
async fn controller_main(controller: Controller) -> anyhow::Result<()> {
    let state = Arc::new(AppState {
        controller,
        sandboxes: DashMap::new(),
        formulas: DashMap::new(),
    });

    // TODO: This should be a unix socket by default (network optional), but it requires quite a bit of code.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3100")
        .await
        .unwrap();
    println!("listening on {}", listener.local_addr().unwrap());

    let app = Router::new()
        .route("/api/1_1/formulas/:suffix", post(create_formula))
        .route(
            "/api/1_1/formulas/:formula/write/:hash/*path",
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
    Path((formula, hash, path)): Path<(String, BlakeId, String)>,
) -> impl IntoResponse {
    let mut headers = HeaderMap::new();
    headers.insert(header::CACHE_CONTROL, "no-store".parse().unwrap());

    let formula = match state.formulas.get(&formula) {
        Some(item) => item,
        None => return (StatusCode::NOT_FOUND, headers),
    };
    let mut formula = formula.lock().await;

    formula.actions.push(Action::UploadFile(hash, path));
    (StatusCode::CREATED, headers)
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

struct BlakeId {
    data: [u8; 32],
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
        base32::decode_into(v, &mut result.data).map_err(|_| {
            E::invalid_value(
                serde::de::Unexpected::Str(v),
                &"a RFC4648 base32-encoded blake3 hash",
            )
        })?;
        Ok(result)
    }
}

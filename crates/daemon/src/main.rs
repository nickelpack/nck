#![feature(never_type)]
#![feature(lazy_cell)]

mod routes;
mod serve;
mod settings;
mod spec_builder;
mod string_types;

use std::sync::Arc;

use axum::{routing::post, Router};
use color_eyre::eyre::{self, Context};
use dashmap::DashMap;
use nck_sandbox::current::Controller;
use settings::Settings;
use spec_builder::SpecBuilder;
use tokio::sync::Mutex;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

struct AppState {
    settings: settings::Settings,
    controller: Mutex<Controller>,
    formulas: DashMap<String, Arc<SpecBuilder>>,
}

fn main() -> eyre::Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let config = config::Config::builder()
        .add_source(
            config::File::new("/etc/nck/config.toml", config::FileFormat::Toml).required(false),
        )
        .add_source(config::Environment::with_prefix("nck").separator("__"))
        .build()
        .wrap_err_with(|| "failed to load configuration")?;

    let config: Settings = config
        .try_deserialize()
        .wrap_err_with(|| "failed to load configuration")?;

    nck_sandbox::current::main(config.clone().into(), move |c| daemon_main(c, config))
        .unwrap_or(Ok(()))
        .wrap_err_with(|| "daemon failed")?;
    Ok(())
}

async fn daemon_main(controller: Controller, settings: Settings) -> eyre::Result<()> {
    color_eyre::install()?;

    let state = Arc::new(AppState {
        settings: settings.clone(),
        controller: Mutex::new(controller),
        formulas: DashMap::new(),
    });

    let app = Router::new()
        .route("/api/1/formulas/:name", post(routes::create_formula))
        .route(
            "/api/1/formulas/:id/write",
            post(routes::formula_write_file),
        )
        .route(
            "/api/1/formulas/:id/copy/*from",
            post(routes::formula_copy_file),
        )
        .route(
            "/api/1/formulas/:id/env/:name",
            post(routes::formula_set_env),
        )
        .with_state(state)
        .layer(TraceLayer::new_for_http());

    serve::serve(settings.tcp.clone(), app)
        .await
        .wrap_err_with(|| "failed to serve requests")?;

    Ok(())
}

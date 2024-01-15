use crate::{settings::Settings, store::Store};

mod build;
mod serve;

#[derive(Debug, Clone)]
struct FrontendState {
    store: Store,
}

pub async fn frontend(store: Store, settings: Settings) -> anyhow::Result<()> {
    let state = FrontendState { store };

    let app = axum::Router::new().nest("/api/1/build", build::create_routes(state.clone()));

    serve::serve(&settings.daemon, app).await?;

    Ok(())
}

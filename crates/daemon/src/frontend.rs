use crate::store::Store;

mod serve;
mod specs;

#[derive(Debug, Clone)]
struct FrontendState {
    store: Store,
}

pub async fn frontend(store: Store) -> anyhow::Result<()> {
    let state = FrontendState { store };

    let app = axum::Router::new().nest("/api/1/spec", specs::create_routes(state.clone()));

    serve::serve(crate::settings::TcpSettings::default(), app).await?;

    Ok(())
}

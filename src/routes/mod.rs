//! Composizione del router HTTP.

mod health;

use axum::Router;
use axum::routing::get;
use tower_http::trace::TraceLayer;

use crate::state::AppState;

/// Costruisce l'albero delle rotte.
///
/// `with_state` inietta lo stato: ogni handler che dichiara `State<AppState>`
/// fra i parametri se lo vede arrivare, senza variabili globali e senza
/// passarlo a mano di funzione in funzione.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health::health))
        // Un layer è un middleware: qui logghiamo metodo, path, status e durata
        // di ogni richiesta. Vale per tutte le rotte definite sopra.
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

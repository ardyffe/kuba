//! Composizione del router HTTP.

mod health;
mod invoices;
mod products;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;

use crate::state::AppState;

/// Tetto alla dimensione di un upload: 20 MB.
///
/// Axum di suo si ferma a 2 MB, che per un PDF con immagini è poco. Un limite
/// però ci vuole: senza, una richiesta enorme potrebbe saturare la memoria del
/// processo. Il limite viene applicato mentre il corpo arriva, non dopo.
const MAX_UPLOAD_BYTES: usize = 20 * 1024 * 1024;

/// Costruisce l'albero delle rotte.
///
/// `with_state` inietta lo stato: ogni handler che dichiara `State<AppState>`
/// fra i parametri se lo vede arrivare, senza variabili globali e senza
/// passarlo a mano di funzione in funzione.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health::health))
        .route(
            "/api/invoices",
            // Stessa URL, due metodi HTTP diversi, due handler diversi.
            post(invoices::upload).get(invoices::list),
        )
        // In axum 0.8 i segnaposto nei path si scrivono `{id}` (nelle versioni
        // precedenti erano `:id`).
        .route("/api/invoices/{id}", get(invoices::detail))
        .route("/api/invoices/{id}/file", get(invoices::download))
        .route("/api/invoices/{id}/retry", post(invoices::retry))
        .route("/api/products", get(products::list))
        .route(
            "/api/products/{id}",
            get(products::detail)
                .put(products::update)
                .delete(products::delete),
        )
        // Il limite di dimensione vale per tutte le rotte qui sopra.
        .layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES))
        // Un layer è un middleware: qui logghiamo metodo, path, status e durata
        // di ogni richiesta.
        .layer(TraceLayer::new_for_http())
        // Il frontend gira su un'altra porta (5173), quindi per il browser è
        // un'altra origine e ogni chiamata sarebbe bloccata senza questo.
        //
        // Permissivo perché in sviluppo: prima di andare in produzione va
        // ristretto all'origine vera del frontend. `Any` sull'origine esclude
        // comunque l'invio di cookie e credenziali, che qui non usiamo.
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .with_state(state)
}

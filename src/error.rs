//! L'errore applicativo e la sua traduzione in risposta HTTP.
//!
//! L'idea: gli handler restituiscono `Result<T, AppError>` e usano `?`. Axum sa
//! trasformare un `AppError` in risposta perché implementiamo `IntoResponse`.
//! Così nessun handler deve più costruire a mano status code e corpo JSON.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// `#[from]` genera `impl From<sqlx::Error> for AppError`.
    /// È questo che fa funzionare `?` su una query: l'operatore converte
    /// automaticamente l'errore nel tipo di errore della funzione.
    #[error("errore del database: {0}")]
    Database(#[from] sqlx::Error),
}

impl AppError {
    /// Come questo errore si presenta al mondo esterno.
    ///
    /// Il messaggio pubblico è volutamente generico: i dettagli di un errore
    /// SQL (nomi di tabelle, vincoli, host) finiscono nei log, non nel corpo
    /// della risposta di un client che non controlliamo.
    fn public(&self) -> (StatusCode, &'static str, &'static str) {
        match self {
            AppError::Database(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "database_error",
                "errore interno, riprova più tardi",
            ),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code, message) = self.public();

        // Il dettaglio completo lo teniamo qui, dove lo vediamo solo noi.
        tracing::error!(error = %self, "richiesta fallita");

        (status, Json(json!({ "error": { "code": code, "message": message } }))).into_response()
    }
}

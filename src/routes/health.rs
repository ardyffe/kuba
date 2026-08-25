//! `GET /api/health` — dice se il servizio è vivo *e* se il DB risponde.

use crate::extract::Json;
use axum::extract::State;
use serde::Serialize;

use crate::error::AppError;
use crate::state::AppState;

/// La forma esatta del JSON di risposta.
///
/// `Serialize` è il trait di serde: il `derive` scrive per noi il codice che
/// trasforma questa struct in JSON. Nessuna stringa costruita a mano.
#[derive(Serialize)]
pub struct HealthResponse {
    status: &'static str,
    database: &'static str,
}

/// Un handler axum è semplicemente una funzione `async`.
///
/// - `State(state)`: *extractor*. Axum lo costruisce prima di chiamarci, e la
///   sintassi con le parentesi è destructuring: tira fuori l'`AppState` dal
///   wrapper `State`.
/// - `Result<Json<_>, AppError>`: il caso felice diventa 200 + JSON, l'errore
///   passa dal nostro `IntoResponse`.
pub async fn health(State(state): State<AppState>) -> Result<Json<HealthResponse>, AppError> {
    // Un ping vero al database: senza questa query risponderemmo "ok" anche con
    // Postgres spento, che è esattamente l'informazione che non ci serve.
    // Il `?` propaga l'errore convertendolo in AppError::Database.
    sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(&state.db)
        .await?;

    Ok(Json(HealthResponse {
        status: "ok",
        database: "up",
    }))
}

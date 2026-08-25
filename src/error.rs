//! L'errore applicativo e la sua traduzione in risposta HTTP.
//!
//! L'idea: gli handler restituiscono `Result<T, AppError>` e usano `?`. Axum sa
//! trasformare un `AppError` in risposta perché implementiamo `IntoResponse`.
//! Così nessun handler deve più costruire a mano status code e corpo JSON.

use axum::Json;
use axum::extract::multipart::MultipartError;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// `#[from]` genera `impl From<sqlx::Error> for AppError`.
    /// È questo che fa funzionare `?` su una query: l'operatore converte
    /// automaticamente l'errore nel tipo di errore della funzione.
    #[error("errore del database: {0}")]
    Database(#[from] sqlx::Error),

    #[error("errore di I/O: {0}")]
    Io(#[from] std::io::Error),

    #[error("multipart malformato: {0}")]
    Multipart(#[from] MultipartError),

    /// Il corpo JSON non è parsabile, o non corrisponde alla struct attesa.
    /// Lo intercetta il nostro estrattore in `extract.rs`.
    #[error("corpo JSON non valido: {0}")]
    JsonBody(#[from] JsonRejection),

    /// Parametri della querystring non validi (es. `?status=pippo`).
    #[error("parametri non validi: {0}")]
    QueryParams(#[from] QueryRejection),

    /// Upload rifiutato: campo mancante, file vuoto, non è un PDF...
    /// Qui la stringa è un messaggio pensato per essere letto dall'utente.
    #[error("upload non valido: {0}")]
    InvalidUpload(String),

    #[error("risorsa non trovata: {0}")]
    NotFound(&'static str),

    /// Il corpo della richiesta è sintatticamente valido ma semanticamente no:
    /// titolo vuoto, nessun campo da aggiornare, valore fuori range.
    #[error("validazione fallita: {0}")]
    Validation(String),

    /// La stessa fattura era già stata caricata (stesso sha256).
    /// Portiamo con noi l'id di quella esistente: è l'informazione che serve
    /// al frontend per portare l'utente sulla fattura giusta.
    #[error("fattura già caricata: {existing_id}")]
    DuplicateInvoice { existing_id: Uuid },
}

impl AppError {
    /// Come questo errore si presenta al mondo esterno.
    ///
    /// I dettagli tecnici (nomi di tabelle, vincoli, percorsi su disco) restano
    /// nei log: al client va solo ciò che gli serve per capire cosa fare.
    fn public(&self) -> (StatusCode, &'static str, String) {
        match self {
            AppError::Database(_) | AppError::Io(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "errore interno, riprova più tardi".to_string(),
            ),
            AppError::Multipart(err) => (
                StatusCode::BAD_REQUEST,
                "invalid_multipart",
                err.body_text(),
            ),
            // Teniamo lo status scelto da axum — distingue il JSON illeggibile
            // (400) dal JSON valido ma con la forma sbagliata (422) — e
            // riscriviamo solo il *formato* della risposta.
            AppError::JsonBody(err) => (err.status(), "invalid_json", err.body_text()),
            AppError::QueryParams(err) => (err.status(), "invalid_query", err.body_text()),
            AppError::InvalidUpload(message) => {
                (StatusCode::BAD_REQUEST, "invalid_upload", message.clone())
            }
            AppError::NotFound(what) => (
                StatusCode::NOT_FOUND,
                "not_found",
                format!("non trovato: {what}"),
            ),
            AppError::Validation(message) => {
                (StatusCode::BAD_REQUEST, "validation_error", message.clone())
            }
            AppError::DuplicateInvoice { existing_id } => (
                StatusCode::CONFLICT,
                "duplicate_invoice",
                format!("questa fattura è già stata caricata (id {existing_id})"),
            ),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code, message) = self.public();

        // Gli errori nostri (5xx) sono bug o guasti: vanno a livello ERROR.
        // Quelli del client (4xx) sono normale funzionamento: WARN basta e
        // avanza, altrimenti i log si riempiono di rumore che non è un problema.
        if status.is_server_error() {
            tracing::error!(error = %self, "richiesta fallita");
        } else {
            tracing::warn!(error = %self, "richiesta rifiutata");
        }

        (
            status,
            Json(json!({ "error": { "code": code, "message": message } })),
        )
            .into_response()
    }
}

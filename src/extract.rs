//! Estrattori personalizzati.

use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{FromRequest, FromRequestParts, Request};
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};

use crate::error::AppError;

/// Il nostro `Json`, che sostituisce quello di axum in ingresso e in uscita.
///
/// # Perché non usiamo direttamente `axum::Json`
///
/// Quando il corpo della richiesta è malformato, `axum::Json` rifiuta la
/// richiesta **prima** che il nostro handler venga chiamato, e risponde con un
/// testo semplice:
///
/// ```text
/// Failed to parse the request body as JSON: EOF while parsing a value
/// ```
///
/// Il resto dell'API risponde invece `{"error": {"code": ..., "message": ...}}`.
/// Un frontend che legge `response.error.code` si romperebbe proprio nei casi
/// di errore, cioè quando ha più bisogno di capire cosa è successo.
///
/// Questo wrapper intercetta il rifiuto e lo fa passare da `AppError`, così il
/// formato della risposta è **uno solo** su tutta l'API.
pub struct Json<T>(pub T);

/// `FromRequest` è il trait che rende un tipo utilizzabile come parametro di un
/// handler: axum lo chiama per costruire il valore a partire dalla richiesta.
///
/// La clausola `where` dice: "vale per ogni `T` che `axum::Json` sa già
/// deserializzare". Non reimplementiamo il parsing, deleghiamo — e ci limitiamo
/// a cambiare il tipo di errore, che è l'unica cosa che ci interessa.
impl<S, T> FromRequest<S> for Json<T>
where
    axum::Json<T>: FromRequest<S, Rejection = JsonRejection>,
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        // Il `?` converte JsonRejection in AppError grazie al `#[from]`
        // dichiarato sulla variante `AppError::JsonBody`.
        let axum::Json(value) = axum::Json::<T>::from_request(req, state).await?;
        Ok(Self(value))
    }
}

/// In uscita non c'è niente da cambiare: deleghiamo ad axum.
/// Averlo permette di usare un solo tipo in entrambe le direzioni.
impl<T: serde::Serialize> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        axum::Json(self.0).into_response()
    }
}

/// Come `Json`, ma per i parametri della querystring.
///
/// Stesso problema, stessa cura: senza questo wrapper, `?status=pippo`
/// risponderebbe in testo semplice invece che nel formato d'errore dell'API.
pub struct Query<T>(pub T);

/// `FromRequestParts` invece di `FromRequest`: la querystring sta negli header
/// e nell'URL, non nel corpo. Un handler può avere **un solo** estrattore che
/// consuma il corpo (e va per ultimo), ma quanti ne vuole di questo tipo.
impl<S, T> FromRequestParts<S> for Query<T>
where
    axum::extract::Query<T>: FromRequestParts<S, Rejection = QueryRejection>,
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let axum::extract::Query(value) =
            axum::extract::Query::<T>::from_request_parts(parts, state).await?;
        Ok(Self(value))
    }
}

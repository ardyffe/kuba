//! Le rotte dei prodotti: lista, dettaglio, modifica, eliminazione.
//!
//! I prodotti non si creano da qui: nascono dall'agente a partire dalle righe
//! di fattura (M5). Questa è la parte di **revisione umana**: si corregge quello
//! che l'AI ha scritto e si butta via quello che ha sbagliato.

use crate::extract::{Json, Query};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;
use uuid::Uuid;

use crate::error::AppError;
use crate::models::product::{Product, ProductStatus, ProductSummary, UpdateProduct};
use crate::state::AppState;

/// Quanti prodotti restituisce la lista se il client non lo specifica.
const DEFAULT_LIMIT: i64 = 50;
/// Tetto invalicabile: senza, un client potrebbe chiedere `?limit=999999`.
const MAX_LIMIT: i64 = 200;
const MAX_TITLE_CHARS: usize = 255;

/// I parametri di `GET /api/products?status=draft&q=lattafa&limit=20&offset=0`.
///
/// Sono tutti opzionali: `Query` li estrae dalla querystring e serde applica
/// i default per quelli assenti.
#[derive(Debug, Deserialize)]
pub struct ListParams {
    status: Option<ProductStatus>,
    /// Ricerca libera su titolo, EAN e SKU.
    q: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

/// `GET /api/products` — la lista, con filtri opzionali.
pub async fn list(
    State(state): State<AppState>,
    Query(params): Query<ListParams>,
) -> Result<Json<Vec<ProductSummary>>, AppError> {
    // `clamp` tiene il valore dentro un intervallo: niente limit negativi,
    // niente richieste da un milione di righe.
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let offset = params.offset.unwrap_or(0).max(0);

    // Filtri opzionali dentro una query **statica**.
    //
    // La tentazione sarebbe costruire l'SQL concatenando stringhe a seconda dei
    // filtri presenti, ma così si perde la verifica a compile time (e si apre la
    // porta alla SQL injection). Il trucco è far decidere al database:
    // `$1 IS NULL OR colonna = $1` significa "se il filtro non c'è, la
    // condizione è sempre vera".
    let products = sqlx::query_as!(
        ProductSummary,
        r#"
        SELECT id, ean, sku, title, brand, price, stock,
               status as "status: ProductStatus", updated_at
        FROM products
        WHERE (($1::product_status IS NULL AND status <> 'deleted') OR status = $1)
          AND ($2::text IS NULL
               OR title ILIKE '%' || $2 || '%'
               OR ean = $2
               OR sku = $2)
        ORDER BY updated_at DESC
        LIMIT $3 OFFSET $4
        "#,
        params.status as Option<ProductStatus>,
        params.q,
        limit,
        offset,
    )
    .fetch_all(&state.db)
    .await?;

    Ok(Json(products))
}

/// `GET /api/products/{id}` — la scheda completa.
pub async fn detail(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Product>, AppError> {
    let product = sqlx::query_as!(
        Product,
        r#"
        SELECT id, ean, sku, title, description, summary, meta_title, meta_description,
               slug, brand, locale, attributes, categories, unit_cost, price, stock,
               status as "status: ProductStatus", created_at, updated_at
        FROM products
        WHERE id = $1
        "#,
        id
    )
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound("prodotto"))?;

    Ok(Json(product))
}

/// `PUT /api/products/{id}` — aggiorna titolo e/o descrizione.
///
/// L'estrattore `Json` va **per ultimo** fra i parametri: consuma il corpo
/// della richiesta, e dopo di lui non resterebbe niente da estrarre.
pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateProduct>,
) -> Result<Json<Product>, AppError> {
    // Il titolo, se presente, viene normalizzato e validato.
    let title = match body.title {
        None => None,
        Some(None) => {
            return Err(AppError::Validation(
                "il titolo non può essere null: è un campo obbligatorio".to_string(),
            ));
        }
        Some(Some(raw)) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Err(AppError::Validation(
                    "il titolo non può essere vuoto".to_string(),
                ));
            }
            // `chars().count()` e non `len()`: `len()` conta i **byte**, e in
            // UTF-8 una "è" ne occupa due. Il limite è sui caratteri.
            if trimmed.chars().count() > MAX_TITLE_CHARS {
                return Err(AppError::Validation(format!(
                    "il titolo supera {MAX_TITLE_CHARS} caratteri"
                )));
            }
            Some(trimmed.to_string())
        }
    };

    // Qui si srotola l'`Option<Option<String>>` in due valori che l'SQL sa usare:
    // un booleano "il campo era presente?" e il valore da scrivere.
    // Una descrizione fatta di soli spazi equivale a nessuna descrizione.
    let (description_present, description) = match body.description {
        None => (false, None),
        Some(value) => (
            true,
            value
                .map(|d| d.trim().to_string())
                .filter(|d| !d.is_empty()),
        ),
    };

    if title.is_none() && !description_present {
        return Err(AppError::Validation(
            "nessun campo da aggiornare: ammessi 'title' e 'description'".to_string(),
        ));
    }

    let product = sqlx::query_as!(
        Product,
        r#"
        UPDATE products
        SET title = COALESCE($2, title),
            description = CASE WHEN $3 THEN $4::text ELSE description END,
            updated_at = now()
        WHERE id = $1 AND status <> 'deleted'
        RETURNING id, ean, sku, title, description, summary, meta_title, meta_description,
                  slug, brand, locale, attributes, categories, unit_cost, price, stock,
                  status as "status: ProductStatus", created_at, updated_at
        "#,
        id,
        title,
        description_present,
        description,
    )
    .fetch_optional(&state.db)
    .await?
    // Nessuna riga aggiornata: o non esiste, o è già stata eliminata. Per chi
    // chiama sono lo stesso caso — la risorsa non è modificabile.
    .ok_or(AppError::NotFound("prodotto"))?;

    tracing::info!(%id, "prodotto aggiornato");

    Ok(Json(product))
}

/// `DELETE /api/products/{id}` — eliminazione **logica**.
///
/// La riga resta: cambia solo lo stato. Serve perché i prodotti sono collegati
/// alle righe di fattura da cui sono nati, e cancellarli davvero perderebbe la
/// storia di cosa è stato acquistato.
///
/// L'operazione è idempotente: eliminare due volte lo stesso prodotto
/// restituisce 204 entrambe le volte, che è il comportamento che ci si aspetta
/// da un DELETE.
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    let deleted = sqlx::query_scalar!(
        r#"
        UPDATE products
        SET status = 'deleted', updated_at = now()
        WHERE id = $1
        RETURNING id
        "#,
        id
    )
    .fetch_optional(&state.db)
    .await?;

    match deleted {
        Some(_) => {
            tracing::info!(%id, "prodotto eliminato (soft delete)");
            // 204: operazione riuscita, non c'è niente da restituire.
            Ok(StatusCode::NO_CONTENT)
        }
        None => Err(AppError::NotFound("prodotto")),
    }
}

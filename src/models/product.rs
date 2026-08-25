//! Il prodotto: modello del database, e i DTO con cui entra ed esce dall'API.
//!
//! Qui si vede una distinzione che terremo per tutto il progetto: **la struct
//! che rappresenta la riga non è la struct che viaggia sull'HTTP**. Sono cose
//! diverse che cambiano per ragioni diverse — aggiungere una colonna interna
//! non deve cambiare il contratto con il frontend, e viceversa.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Deserializer, Serialize};
use uuid::Uuid;

/// Lo stato di pubblicazione di un prodotto.
///
/// `deleted` esiste perché **non cancelliamo davvero le righe**: un prodotto
/// eliminato è collegato a righe di fattura, e cancellarlo perderebbe la
/// tracciabilità di cosa è stato acquistato. Si nasconde, non si distrugge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "product_status", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ProductStatus {
    Draft,
    Published,
    Deleted,
}

/// Il prodotto completo, come esce da `GET /api/products/{id}`.
#[derive(Debug, Serialize)]
pub struct Product {
    pub id: Uuid,
    pub ean: Option<String>,
    pub sku: Option<String>,
    pub title: String,
    pub description: Option<String>,
    pub summary: Option<String>,
    pub meta_title: Option<String>,
    pub meta_description: Option<String>,
    pub slug: Option<String>,
    pub brand: Option<String>,
    pub locale: String,
    /// Le feature della scheda (note olfattive, famiglia, ml...). Resta
    /// `serde_json::Value` perché la forma cambia da categoria a categoria.
    pub attributes: serde_json::Value,
    pub categories: Vec<String>,
    /// `Decimal` e non `f64`: sono soldi. Vedi il commento nella migration.
    pub unit_cost: Option<Decimal>,
    pub price: Option<Decimal>,
    pub stock: i32,
    pub status: ProductStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// La versione ridotta, per le liste.
///
/// Il catalogo del cliente ha ~3000 prodotti: mandare `description` (HTML lungo)
/// e `attributes` per ognuno significherebbe megabyte di JSON che il frontend
/// non usa per disegnare una tabella. Una struct diversa, non un campo opzionale.
#[derive(Debug, Serialize)]
pub struct ProductSummary {
    pub id: Uuid,
    pub ean: Option<String>,
    pub sku: Option<String>,
    pub title: String,
    pub brand: Option<String>,
    pub price: Option<Decimal>,
    pub stock: i32,
    pub status: ProductStatus,
    pub updated_at: DateTime<Utc>,
}

/// Il corpo di `PUT /api/products/{id}`.
///
/// # Il problema degli aggiornamenti parziali
///
/// Con un `Option<String>` non si distinguono due richieste diverse:
///
/// ```json
/// { "title": "Nuovo" }                        // description: non toccarla
/// { "title": "Nuovo", "description": null }   // description: svuotala
/// ```
///
/// In entrambi i casi serde produrrebbe `None`, e il campo verrebbe trattato
/// allo stesso modo. Sono però due intenzioni opposte.
///
/// La soluzione è `Option<Option<String>>`, dove il livello esterno significa
/// "il campo era presente?" e quello interno "conteneva un valore?":
///
/// | JSON ricevuto | Valore | Significato |
/// |---|---|---|
/// | campo assente | `None` | non toccare |
/// | `"description": null` | `Some(None)` | metti a NULL |
/// | `"description": "x"` | `Some(Some("x"))` | scrivi "x" |
///
/// Serve `deserialize_with`: da solo serde collasserebbe comunque `null` in
/// `None`, perdendo di nuovo la distinzione.
///
/// Anche `title` usa la stessa forma, ma per il motivo opposto: nel database è
/// `NOT NULL`, quindi `Some(None)` è un caso che vogliamo **riconoscere per
/// rifiutarlo** con un messaggio esplicito ("il titolo non può essere null").
/// Con un `Option<String>` semplice, `{"title": null}` sarebbe indistinguibile
/// da un campo assente e l'utente riceverebbe un errore che parla d'altro.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateProduct {
    #[serde(default, deserialize_with = "present_option")]
    pub title: Option<Option<String>>,

    #[serde(default, deserialize_with = "present_option")]
    pub description: Option<Option<String>>,
}

/// Distingue "campo assente" da "campo presente e null".
///
/// Il trucco: questa funzione viene chiamata **solo se il campo è presente**
/// nel JSON. Quindi qualunque cosa arrivi la avvolgiamo in `Some`, e l'assenza
/// resta gestita da `#[serde(default)]`, che produce `None`.
fn present_option<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(deserializer).map(Some)
}

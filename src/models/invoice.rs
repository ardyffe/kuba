//! La fattura: il tipo Rust che corrisponde alla riga della tabella `invoices`.

use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;
use uuid::Uuid;

/// Lo stato di lavorazione di una fattura.
///
/// Questa è la parte che rende utile Rust in questo progetto. Tre derive
/// lavorano insieme su un tipo solo:
///
/// - `sqlx::Type` + `#[sqlx(type_name = ...)]` lo lega al tipo enum di Postgres.
///   `rename_all = "snake_case"` traduce `InProgress` ⇄ `'in_progress'`.
/// - `Serialize` lo trasforma in stringa JSON per il frontend.
/// - `Copy` perché sono quattro varianti senza dati dentro: occupa un byte,
///   copiarlo costa meno che gestire un prestito.
///
/// Il guadagno vero arriva quando scriveremo il worker: ogni `match` su questo
/// enum deve coprire tutte le varianti, altrimenti il codice **non compila**.
/// Aggiungere domani uno stato `Cancelled` non sarà un bug silenzioso: sarà una
/// lista di errori di compilazione che ti indica esattamente cosa aggiornare.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, sqlx::Type)]
#[sqlx(type_name = "invoice_status", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum InvoiceStatus {
    Pending,
    InProgress,
    Succeeded,
    Failed,
}

/// Una fattura come la vede il client.
///
/// Nota cosa **non** c'è: `storage_path`. Il percorso su disco è un dettaglio
/// interno del server, e mandarlo in giro regalerebbe informazioni sulla
/// struttura del filesystem a chi non ne ha bisogno. Il client conosce l'id,
/// e con quello scarica il file dalla rotta dedicata.
#[derive(Debug, Serialize)]
pub struct Invoice {
    pub id: Uuid,
    pub original_filename: String,
    pub size_bytes: i64,
    pub sha256: String,
    pub supplier_name: Option<String>,
    pub invoice_number: Option<String>,
    pub invoice_date: Option<NaiveDate>,
    pub status: InvoiceStatus,
    pub error_message: Option<String>,
    pub uploaded_at: DateTime<Utc>,
}

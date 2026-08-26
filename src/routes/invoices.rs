//! Le rotte delle fatture: upload, lista, dettaglio, download del PDF.

use crate::extract::Json;
use axum::body::Bytes;
use axum::extract::{Multipart, Path, State};
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::config::Config;
use crate::error::AppError;
use crate::models::invoice::{Invoice, InvoiceStatus};
use crate::models::line_item::{InvoiceLineItem, LineItemAction, LineItemKind, LineItemStatus};
use crate::state::AppState;

/// I primi byte di ogni PDF valido. È il modo giusto di riconoscere un file:
/// il `Content-Type` e l'estensione li sceglie il client, e il client mente.
const PDF_MAGIC: &[u8] = b"%PDF-";

/// La risposta all'upload: solo quello che serve al frontend per proseguire.
#[derive(Serialize)]
pub struct UploadAccepted {
    id: Uuid,
    status: InvoiceStatus,
}

/// `POST /api/invoices` — riceve un form multipart con un campo `file`.
///
/// Risponde **202 Accepted**, non 200: il significato è "l'ho presa in carico,
/// il lavoro vero non è ancora finito". Da M3 quel lavoro sarà l'agente.
pub async fn upload(
    State(state): State<AppState>,
    multipart: Multipart,
) -> Result<(StatusCode, Json<UploadAccepted>), AppError> {
    let (filename, data) = read_pdf_field(multipart).await?;

    // Impronta del contenuto. Due file con lo stesso nome ma contenuto diverso
    // sono fatture diverse; lo stesso contenuto con nomi diversi è un doppione.
    let sha256 = hex::encode(Sha256::digest(&data));

    // Generiamo l'id qui invece di lasciarlo fare al DEFAULT di Postgres:
    // ci serve *prima* dell'INSERT, perché è il nome del file su disco.
    let id = Uuid::new_v4();
    // Nel database va la chiave logica (`invoices/{id}.pdf`); il percorso vero
    // su disco lo ricaviamo da lei ogni volta che serve.
    let storage_key = Config::invoice_key(&id);
    let path = state.config.resolve(&storage_key);
    let size_bytes = data.len() as i64;

    // L'ordine conta: prima la riga nel database, poi il file.
    // Se l'INSERT fallisce non abbiamo scritto niente su disco; se fallisce la
    // scrittura, cancelliamo la riga. L'alternativa (file per primo) lascerebbe
    // file orfani che nessuno sa più a cosa appartengono.
    //
    // `ON CONFLICT (sha256) DO NOTHING ... RETURNING id` restituisce una riga
    // solo se l'inserimento è avvenuto davvero: è il database a decidere sui
    // doppioni, in modo atomico. Un controllo "SELECT prima, INSERT poi" avrebbe
    // una finestra fra i due in cui due upload simultanei passano entrambi.
    let inserted = sqlx::query_scalar!(
        r#"
        INSERT INTO invoices (id, original_filename, storage_path, mime_type, size_bytes, sha256)
        VALUES ($1, $2, $3, $4, $5, $6)
        ON CONFLICT (sha256) DO NOTHING
        RETURNING id
        "#,
        id,
        filename,
        storage_key,
        "application/pdf",
        size_bytes,
        sha256,
    )
    .fetch_optional(&state.db)
    .await?;

    if inserted.is_none() {
        let existing_id = sqlx::query_scalar!("SELECT id FROM invoices WHERE sha256 = $1", sha256)
            .fetch_one(&state.db)
            .await?;
        return Err(AppError::DuplicateInvoice { existing_id });
    }

    if let Err(err) = tokio::fs::write(&path, &data).await {
        // Compensazione: la riga senza il suo file non deve restare.
        // `.ok()` scarta di proposito l'esito della DELETE — stiamo già
        // tornando un errore, e il fallimento della pulizia lo logghiamo qui.
        if let Err(cleanup) = sqlx::query!("DELETE FROM invoices WHERE id = $1", id)
            .execute(&state.db)
            .await
        {
            tracing::error!(%id, error = %cleanup, "riga orfana: DELETE di compensazione fallita");
        }
        return Err(AppError::Io(err));
    }

    tracing::info!(%id, filename, size_bytes, "fattura caricata");

    Ok((
        StatusCode::ACCEPTED,
        Json(UploadAccepted {
            id,
            status: InvoiceStatus::Pending,
        }),
    ))
}

/// `GET /api/invoices` — le fatture, dalla più recente.
pub async fn list(State(state): State<AppState>) -> Result<Json<Vec<Invoice>>, AppError> {
    // `query_as!` mappa il risultato direttamente sulla struct `Invoice`.
    //
    // La sintassi `status as "status: InvoiceStatus"` serve a dire a sqlx:
    // "questa colonna è l'enum Postgres, trattala come il mio enum Rust".
    // Senza, la macro si fermerebbe non sapendo come convertire un tipo
    // definito da noi.
    let invoices = sqlx::query_as!(
        Invoice,
        r#"
        SELECT id, original_filename, size_bytes, sha256, supplier_name, invoice_number,
               invoice_date, currency, total_amount,
               status as "status: InvoiceStatus", error_message, uploaded_at
        FROM invoices
        ORDER BY uploaded_at DESC
        "#
    )
    .fetch_all(&state.db)
    .await?;

    Ok(Json(invoices))
}

/// La fattura con dentro le sue righe.
///
/// `#[serde(flatten)]` fonde i campi di `Invoice` al livello superiore del JSON,
/// invece di annidarli sotto una chiave `invoice`. Il client vede un oggetto
/// solo — `{ "id": ..., "status": ..., "lines": [...] }` — e la lista delle
/// fatture e il dettaglio parlano la stessa lingua.
#[derive(Serialize)]
pub struct InvoiceDetail {
    #[serde(flatten)]
    invoice: Invoice,
    lines: Vec<InvoiceLineItem>,
}

/// `GET /api/invoices/{id}` — una fattura con le sue righe estratte.
pub async fn detail(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<InvoiceDetail>, AppError> {
    let invoice = sqlx::query_as!(
        Invoice,
        r#"
        SELECT id, original_filename, size_bytes, sha256, supplier_name, invoice_number,
               invoice_date, currency, total_amount,
               status as "status: InvoiceStatus", error_message, uploaded_at
        FROM invoices
        WHERE id = $1
        "#,
        id
    )
    .fetch_optional(&state.db)
    .await?
    // `fetch_optional` dà `Option<Invoice>`: qui trasformiamo il caso `None`
    // nel nostro 404. `ok_or` converte un Option in un Result, e il `?` fa il resto.
    .ok_or(AppError::NotFound("fattura"))?;

    // Due query invece di una JOIN: la JOIN ripeterebbe i dati di testata su
    // ogni riga e ci costringerebbe a ricomporli a mano. Con poche decine di
    // righe per fattura, due andate al database sono più semplici e più chiare.
    let lines = sqlx::query_as!(
        InvoiceLineItem,
        r#"
        SELECT id, line_no, raw_text, description, ean, supplier_sku, quantity,
               unit_price, amount,
               kind as "kind: LineItemKind",
               action as "action: LineItemAction",
               status as "status: LineItemStatus",
               matched_product_id, error_message
        FROM invoice_line_items
        WHERE invoice_id = $1
        ORDER BY line_no
        "#,
        id
    )
    .fetch_all(&state.db)
    .await?;

    Ok(Json(InvoiceDetail { invoice, lines }))
}

/// `POST /api/invoices/{id}/retry` — rimette in coda una fattura fallita.
///
/// Azzera i tentativi: è una decisione umana, non un ritentativo automatico.
/// Chi preme il pulsante ha presumibilmente sistemato la causa del problema.
///
/// Il `WHERE status = 'failed'` fa da guardia: rimettere in coda una fattura
/// già completata la rilavorerebbe da capo, e una `in_progress` finirebbe
/// lavorata due volte in parallelo.
pub async fn retry(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<(StatusCode, Json<UploadAccepted>), AppError> {
    let requeued = sqlx::query_scalar!(
        r#"
        UPDATE invoices
        SET status = 'pending', attempts = 0, error_message = NULL,
            next_attempt_at = NULL, started_at = NULL, finished_at = NULL
        WHERE id = $1 AND status = 'failed'
        RETURNING id
        "#,
        id
    )
    .fetch_optional(&state.db)
    .await?;

    match requeued {
        Some(id) => {
            tracing::info!(%id, "fattura rimessa in coda");
            // 202 come l'upload, e per lo stesso motivo: la fattura è in coda,
            // il lavoro deve ancora avvenire.
            Ok((
                StatusCode::ACCEPTED,
                Json(UploadAccepted {
                    id,
                    status: InvoiceStatus::Pending,
                }),
            ))
        }
        // Niente riga aggiornata: o non esiste, o non è in stato `failed`.
        // Distinguiamo i due casi, perché all'utente servono risposte diverse.
        None => {
            let current = sqlx::query_scalar!(
                r#"SELECT status as "status: InvoiceStatus" FROM invoices WHERE id = $1"#,
                id
            )
            .fetch_optional(&state.db)
            .await?
            .ok_or(AppError::NotFound("fattura"))?;

            Err(AppError::Validation(format!(
                "solo una fattura fallita può essere rimessa in coda (stato attuale: {})",
                current.as_str()
            )))
        }
    }
}

/// `GET /api/invoices/{id}/file` — restituisce il PDF originale.
pub async fn download(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let record = sqlx::query!(
        "SELECT storage_path, original_filename FROM invoices WHERE id = $1",
        id
    )
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound("fattura"))?;

    // Le fatture pesano decine di KB: leggerle in memoria è semplice e va bene.
    // Se un domani caricassimo file da centinaia di MB, qui passeremmo a uno
    // stream (`tokio_util::io::ReaderStream` + `Body::from_stream`) per non
    // tenere l'intero file in RAM per ogni download simultaneo.
    let data = tokio::fs::read(state.config.resolve(&record.storage_path)).await?;

    let headers = [
        (header::CONTENT_TYPE, "application/pdf".to_string()),
        (
            header::CONTENT_DISPOSITION,
            format!(
                "inline; filename=\"{}\"",
                sanitize_filename(&record.original_filename)
            ),
        ),
    ];

    Ok((headers, data))
}

/// Estrae il campo `file` dal multipart e verifica che sia davvero un PDF.
///
/// `Multipart` è un flusso: i campi arrivano uno alla volta dalla rete, per
/// questo va consumato con `.await` a ogni passo e non è un semplice `HashMap`.
async fn read_pdf_field(mut multipart: Multipart) -> Result<(String, Bytes), AppError> {
    while let Some(field) = multipart.next_field().await? {
        // Nome e filename vanno copiati *prima* di leggere i byte: `field.bytes()`
        // consuma il campo, e dopo non esisterebbe più niente da cui prestarli.
        let name = field.name().map(str::to_owned);
        let filename = field.file_name().map(str::to_owned);

        if name.as_deref() != Some("file") {
            continue;
        }

        let filename = filename.ok_or_else(|| {
            AppError::InvalidUpload("il campo 'file' non contiene un file".to_string())
        })?;

        let data = field.bytes().await?;

        if data.is_empty() {
            return Err(AppError::InvalidUpload("il file è vuoto".to_string()));
        }
        if !data.starts_with(PDF_MAGIC) {
            return Err(AppError::InvalidUpload(format!(
                "'{filename}' non è un PDF: sono accettate solo fatture in formato PDF"
            )));
        }

        return Ok((filename, data));
    }

    Err(AppError::InvalidUpload(
        "manca il campo 'file' nel form".to_string(),
    ))
}

/// Ripulisce il nome del file prima di metterlo in un header HTTP.
///
/// Il nome arriva dal client: se contenesse virgolette o un a capo potrebbe
/// chiudere il valore dell'header e iniettarne altri (*header injection*).
/// Teniamo solo caratteri innocui.
fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | ' '))
        .take(100)
        .collect();

    if cleaned.trim().is_empty() {
        "fattura.pdf".to_string()
    } else {
        cleaned
    }
}

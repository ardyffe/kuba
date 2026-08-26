//! Il worker: prende le fatture in coda e le lavora.
//!
//! # Perché la coda vive in Postgres
//!
//! L'alternativa ovvia sarebbe lanciare un task al momento dell'upload
//! (`tokio::spawn` dentro l'handler). Funziona finché non succede niente: se il
//! processo muore, o viene riavviato per un deploy, quel lavoro **sparisce** e
//! nessuno sa che esisteva.
//!
//! Mettendo la coda in una tabella, invece, lo stato sopravvive al processo: al
//! riavvio le fatture `pending` sono ancora lì. In più è lo stesso stato che il
//! frontend mostra all'utente, quindi non c'è niente da tenere sincronizzato.
//!
//! # Il ciclo di vita
//!
//! ```text
//!   upload ──► pending ──► in_progress ──┬──► succeeded
//!                 ▲                      │
//!                 └── (retry, backoff) ◄─┴──► failed ──► (POST /retry)
//! ```

use std::time::Duration;

use chrono::NaiveDate;
use rust_decimal::Decimal;
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::claude::{ClaudeError, ExtractedInvoice};
use crate::models::line_item::LineItemKind;
use crate::state::AppState;

/// Ogni quanto il worker torna a chiedere al database se c'è lavoro.
///
/// Due secondi sono un compromesso: abbastanza reattivo per un umano che
/// guarda la pagina, abbastanza raro da non pesare. Da M7, con le notifiche
/// `LISTEN/NOTIFY` di Postgres, il polling diventerà solo una rete di sicurezza.
const IDLE_POLL: Duration = Duration::from_secs(2);

/// Quanto aspettare se è il *database* a non rispondere: inutile martellarlo.
const ERROR_POLL: Duration = Duration::from_secs(10);

/// Dopo quanti tentativi una fattura viene dichiarata fallita.
const MAX_ATTEMPTS: i32 = 3;

/// Base del backoff esponenziale: 30s dopo il 1° errore, 60s dopo il 2°.
const BACKOFF_BASE_SECS: u32 = 30;

/// Una fattura presa in carico dal worker.
struct Job {
    id: Uuid,
    original_filename: String,
    storage_path: String,
    attempts: i32,
}

/// Gli errori della lavorazione.
///
/// Sono separati da `AppError` di proposito: quelli sono errori *di una
/// richiesta HTTP*, questi sono errori *di un lavoro in background*. Non hanno
/// uno status code, hanno un messaggio che finisce nella colonna
/// `error_message` e che l'utente leggerà nella pagina della fattura.
#[derive(Debug, thiserror::Error)]
enum JobError {
    #[error("il file della fattura non è leggibile: {0}")]
    FileUnreadable(String),

    #[error("il file non è un PDF valido")]
    NotAPdf,

    #[error("estrazione fallita: {0}")]
    Extraction(#[from] ClaudeError),

    #[error("nessuna riga estratta dalla fattura")]
    NothingExtracted,

    #[error("errore del database: {0}")]
    Database(#[from] sqlx::Error),
}

impl JobError {
    /// Ha senso riprovare?
    ///
    /// Distinzione che prima non facevamo, e che ora conta: con l'API a
    /// consumo, ritentare tre volte un errore che è per sua natura permanente
    /// (chiave sbagliata, file cancellato) è tempo e denaro buttati.
    fn is_retryable(&self) -> bool {
        match self {
            // Il file non ricomparirà da solo, e un PDF corrotto resta corrotto.
            JobError::FileUnreadable(_) | JobError::NotAPdf => false,
            // Qui decide il client: 429 e 5xx sì, 400 e 401 no.
            JobError::Extraction(err) => err.is_retryable(),
            // Può essere una risposta sfortunata: un tentativo in più è onesto.
            JobError::NothingExtracted => true,
            JobError::Database(_) => true,
        }
    }
}

/// Avvia il ciclo del worker. Ritorna quando arriva la cancellazione.
///
/// Questa funzione viene passata a `tokio::spawn` in `main.rs`, quindi gira su
/// un task suo: l'API continua a rispondere mentre il worker lavora.
pub async fn run(state: AppState, shutdown: CancellationToken) {
    tracing::info!("worker avviato");

    // Prima di tutto, rimettiamo in coda i lavori rimasti a metà.
    if let Err(err) = requeue_stale(&state.db).await {
        tracing::error!(error = %err, "impossibile recuperare i lavori interrotti");
    }

    while !shutdown.is_cancelled() {
        let pause = match claim_next(&state.db).await {
            Ok(Some(job)) => {
                process(&state, job).await;
                // C'era lavoro: riproviamo subito, potrebbe essercene altro.
                Duration::ZERO
            }
            Ok(None) => IDLE_POLL,
            Err(err) => {
                tracing::error!(error = %err, "claim fallito");
                ERROR_POLL
            }
        };

        if pause.is_zero() {
            continue;
        }

        // `select!` aspetta il primo dei due rami che si completa. Così
        // l'attesa è interrompibile: allo spegnimento non restiamo fermi due
        // secondi buoni prima di accorgercene.
        //
        // Nota dove *non* c'è un select: attorno a `process`. Un lavoro
        // iniziato deve poter finire, altrimenti lo spegnimento lascerebbe una
        // fattura bloccata in `in_progress`. La cancellazione si controlla fra
        // un lavoro e l'altro, non dentro.
        tokio::select! {
            _ = shutdown.cancelled() => break,
            _ = tokio::time::sleep(pause) => {}
        }
    }

    tracing::info!("worker fermato");
}

/// Rimette in coda le fatture rimaste `in_progress` da un'esecuzione precedente.
///
/// Se il processo muore mentre lavora una fattura, quella riga resta
/// `in_progress` per sempre: nessuno la riprenderà, perché il claim cerca solo
/// le `pending`.
///
/// Questa versione assume **un solo worker attivo**: all'avvio, qualunque
/// `in_progress` è per forza un residuo. Con più istanze in parallelo servirebbe
/// distinguere "in lavorazione da qualcun altro adesso" da "abbandonata", e la
/// soluzione standard è un lease: una colonna con la scadenza della presa in
/// carico, che il worker rinnova mentre lavora.
async fn requeue_stale(db: &PgPool) -> Result<(), sqlx::Error> {
    let requeued = sqlx::query_scalar!(
        r#"
        UPDATE invoices
        SET status = 'pending', started_at = NULL
        WHERE status = 'in_progress'
        RETURNING id
        "#
    )
    .fetch_all(db)
    .await?;

    if !requeued.is_empty() {
        tracing::warn!(count = requeued.len(), "lavori interrotti rimessi in coda");
    }

    Ok(())
}

/// Prende la prossima fattura da lavorare, se c'è.
///
/// # `FOR UPDATE SKIP LOCKED`
///
/// È la parte importante. `FOR UPDATE` blocca le righe selezionate fino a fine
/// transazione; `SKIP LOCKED` dice "se una riga è già bloccata da qualcun altro,
/// saltala invece di aspettare".
///
/// Insieme, sono ciò che rende sicuro avere più worker: due processi che fanno
/// questa query nello stesso istante prendono due fatture **diverse**, senza
/// lock espliciti e senza rischio che la stessa fattura venga lavorata due
/// volte. È il database a fare da arbitro.
///
/// Il `WHERE` sul tempo implementa il backoff: una fattura che ha appena
/// fallito ha `next_attempt_at` nel futuro e resta invisibile fino ad allora.
async fn claim_next(db: &PgPool) -> Result<Option<Job>, sqlx::Error> {
    let job = sqlx::query_as!(
        Job,
        r#"
        WITH prossima AS (
            SELECT id
            FROM invoices
            WHERE status = 'pending'
              AND (next_attempt_at IS NULL OR next_attempt_at <= now())
            ORDER BY uploaded_at
            FOR UPDATE SKIP LOCKED
            LIMIT 1
        )
        UPDATE invoices
        SET status = 'in_progress',
            started_at = now(),
            attempts = attempts + 1
        FROM prossima
        WHERE invoices.id = prossima.id
        RETURNING invoices.id, invoices.original_filename,
                  invoices.storage_path, invoices.attempts
        "#
    )
    .fetch_optional(db)
    .await?;

    Ok(job)
}

/// Lavora una fattura e ne registra l'esito.
///
/// Non restituisce `Result` di proposito: qualunque cosa accada, lo stato della
/// fattura **deve** essere scritto. Un errore che si propaga via `?` lascerebbe
/// la riga in `in_progress` per sempre.
async fn process(state: &AppState, job: Job) {
    let id = job.id;
    tracing::info!(%id, file = job.original_filename, attempt = job.attempts, "lavorazione avviata");

    match run_pipeline(state, &job).await {
        Ok(()) => {
            if let Err(err) = mark_succeeded(&state.db, id).await {
                tracing::error!(%id, error = %err, "impossibile segnare la fattura come completata");
            } else {
                tracing::info!(%id, "lavorazione completata");
            }
        }
        Err(err) => {
            let message = err.to_string();
            // Un altro tentativo ha senso solo se ne restano **e** se l'errore
            // è di quelli che possono andare diversamente.
            let retry_in =
                (job.attempts < MAX_ATTEMPTS && err.is_retryable()).then(|| backoff(job.attempts));

            match retry_in {
                Some(delay) => {
                    tracing::warn!(%id, error = %message, attempt = job.attempts, retry_in_secs = delay, "lavorazione fallita, riprovo");
                    if let Err(err) = schedule_retry(&state.db, id, &message, delay).await {
                        tracing::error!(%id, error = %err, "impossibile programmare il nuovo tentativo");
                    }
                }
                None => {
                    tracing::error!(%id, error = %message, attempts = job.attempts, "lavorazione fallita definitivamente");
                    if let Err(err) = mark_failed(&state.db, id, &message).await {
                        tracing::error!(%id, error = %err, "impossibile segnare la fattura come fallita");
                    }
                }
            }
        }
    }
}

/// La pipeline vera e propria.
///
/// Oggi fa solo i controlli reali che sappiamo già fare; i passi dell'agente
/// arrivano in M4 e M5, e si innestano qui senza toccare niente di quanto sta
/// sopra. È il senso di questa milestone: prima l'impalcatura, verificata, poi
/// il contenuto.
async fn run_pipeline(state: &AppState, job: &Job) -> Result<(), JobError> {
    // 1. Il file esiste ed è leggibile?
    let path = state.config.resolve(&job.storage_path);
    let data = tokio::fs::read(&path)
        .await
        .map_err(|err| JobError::FileUnreadable(err.to_string()))?;

    // 2. È davvero un PDF? (già verificato all'upload, ma il file su disco
    //    potrebbe essere stato toccato nel frattempo)
    if !data.starts_with(b"%PDF-") {
        return Err(JobError::NotAPdf);
    }

    tracing::debug!(id = %job.id, bytes = data.len(), "PDF letto");

    // 3. Estrazione delle righe: il PDF va al modello, torna JSON strutturato.
    let extracted = state.claude.extract_invoice(&data).await?;

    if extracted.lines.is_empty() {
        return Err(JobError::NothingExtracted);
    }

    // 4. Persistenza di testata e righe, in una transazione sola.
    let saved = store_extraction(&state.db, job.id, &extracted).await?;
    tracing::info!(id = %job.id, righe = saved, "righe salvate");

    // 5. Match sugli EAN già a catalogo — M5.
    simulate("match sul catalogo").await;

    // 6. Arricchimento e creazione delle schede — M5.
    simulate("arricchimento e creazione prodotti").await;

    Ok(())
}

/// Segnaposto per i passi non ancora implementati.
async fn simulate(step: &str) {
    tracing::debug!(step, "passo simulato (stub)");
    tokio::time::sleep(Duration::from_millis(300)).await;
}

/// Scrive testata e righe della fattura.
///
/// # Perché una transazione
///
/// Le due scritture sono una cosa sola dal punto di vista logico: una fattura
/// con la testata aggiornata ma senza righe è uno stato che non deve esistere
/// nemmeno per un istante. `BEGIN ... COMMIT` fa sì che chi legge veda o il
/// prima o il dopo, mai il mezzo — e se qualcosa fallisce a metà, il database
/// torna da solo al punto di partenza.
///
/// # Perché UPSERT e non INSERT
///
/// Al secondo tentativo della stessa fattura le righe verrebbero inserite di
/// nuovo. `ON CONFLICT (invoice_id, line_no) DO UPDATE` fa sì che la riga 3
/// resti la riga 3, aggiornata: l'estrazione diventa **idempotente**, si può
/// ripetere quante volte si vuole senza sporcare i dati.
async fn store_extraction(
    db: &PgPool,
    invoice_id: Uuid,
    extracted: &ExtractedInvoice,
) -> Result<usize, sqlx::Error> {
    let mut tx = db.begin().await?;

    sqlx::query!(
        r#"
        UPDATE invoices
        SET supplier_name = $2, invoice_number = $3, invoice_date = $4,
            currency = $5, total_amount = $6
        WHERE id = $1
        "#,
        invoice_id,
        extracted.supplier_name,
        extracted.invoice_number,
        parse_date(extracted.invoice_date.as_deref(), "invoice_date"),
        extracted.currency,
        parse_decimal(extracted.total_amount.as_deref(), "total_amount"),
    )
    .execute(&mut *tx)
    .await?;

    for line in &extracted.lines {
        sqlx::query!(
            r#"
            INSERT INTO invoice_line_items
                (invoice_id, line_no, raw_text, description, ean, supplier_sku,
                 quantity, unit_price, amount, kind)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            ON CONFLICT (invoice_id, line_no) DO UPDATE SET
                raw_text = EXCLUDED.raw_text,
                description = EXCLUDED.description,
                ean = EXCLUDED.ean,
                supplier_sku = EXCLUDED.supplier_sku,
                quantity = EXCLUDED.quantity,
                unit_price = EXCLUDED.unit_price,
                amount = EXCLUDED.amount,
                kind = EXCLUDED.kind,
                status = 'pending',
                error_message = NULL
            "#,
            invoice_id,
            line.line_no,
            line.raw_text,
            line.description,
            line.ean,
            line.supplier_sku,
            line.quantity,
            parse_decimal(line.unit_price.as_deref(), "unit_price"),
            parse_decimal(line.amount.as_deref(), "amount"),
            LineItemKind::parse(&line.kind) as LineItemKind,
        )
        .execute(&mut *tx)
        .await?;
    }

    // Fino a qui niente è visibile agli altri. È questa riga a renderlo vero.
    tx.commit().await?;

    Ok(extracted.lines.len())
}

/// Converte un importo testuale in `Decimal`.
///
/// Un valore malformato **non** fa fallire l'estrazione: diventa `NULL` e
/// lascia una riga nei log. Buttare via 39 righe corrette perché la 17ª ha un
/// prezzo scritto male sarebbe un pessimo affare — e il campo `raw_text`
/// conserva comunque l'originale per chi rivede.
fn parse_decimal(raw: Option<&str>, field: &str) -> Option<Decimal> {
    let raw = raw?.trim();
    if raw.is_empty() {
        return None;
    }

    match Decimal::from_str_exact(raw) {
        Ok(value) => Some(value),
        Err(err) => {
            tracing::warn!(field, raw, error = %err, "importo non interpretabile, salvo NULL");
            None
        }
    }
}

fn parse_date(raw: Option<&str>, field: &str) -> Option<NaiveDate> {
    let raw = raw?.trim();
    if raw.is_empty() {
        return None;
    }

    match NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
        Ok(value) => Some(value),
        Err(err) => {
            tracing::warn!(field, raw, error = %err, "data non interpretabile, salvo NULL");
            None
        }
    }
}

/// Backoff esponenziale: 30s dopo il primo errore, 60s dopo il secondo.
///
/// Riprovare subito è quasi sempre inutile — se l'API remota è giù, lo è ancora
/// un millisecondo dopo — e trasforma un guasto passeggero in un martellamento.
fn backoff(attempts: i32) -> i64 {
    let exponent = (attempts - 1).max(0) as u32;
    i64::from(BACKOFF_BASE_SECS * 2u32.pow(exponent.min(10)))
}

async fn mark_succeeded(db: &PgPool, id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"
        UPDATE invoices
        SET status = 'succeeded', finished_at = now(), error_message = NULL,
            next_attempt_at = NULL
        WHERE id = $1
        "#,
        id
    )
    .execute(db)
    .await?;
    Ok(())
}

async fn schedule_retry(
    db: &PgPool,
    id: Uuid,
    message: &str,
    delay_secs: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"
        UPDATE invoices
        SET status = 'pending',
            started_at = NULL,
            error_message = $2,
            next_attempt_at = now() + make_interval(secs => $3)
        WHERE id = $1
        "#,
        id,
        message,
        delay_secs as f64,
    )
    .execute(db)
    .await?;
    Ok(())
}

async fn mark_failed(db: &PgPool, id: Uuid, message: &str) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"
        UPDATE invoices
        SET status = 'failed', finished_at = now(), error_message = $2,
            next_attempt_at = NULL
        WHERE id = $1
        "#,
        id,
        message
    )
    .execute(db)
    .await?;
    Ok(())
}

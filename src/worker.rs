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

use futures::StreamExt;

use crate::claude::{ClaudeError, EnrichedProduct, ExtractedInvoice};
use crate::models::line_item::{LineItemAction, LineItemKind, LineItemStatus};
use crate::models::product::ProductStatus;
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

/// Quante schede prodotto si generano in parallelo.
///
/// Non è un numero preso a caso: ogni scheda sono ricerche web più una risposta
/// lunga, e una fattura da 40 righe nuove lanciate tutte insieme prenderebbe un
/// 429. Tre alla volta tengono occupata la rete senza esagerare.
const ENRICH_CONCURRENCY: usize = 3;

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

    // 5. Match sugli EAN già a catalogo: decide cosa fare di ogni riga.
    let da_creare = match_lines(&state.db, job.id).await?;
    tracing::info!(id = %job.id, da_creare = da_creare.len(), "match completato");

    // 6. Arricchimento e creazione delle schede, per le sole righe nuove.
    if !da_creare.is_empty() {
        enrich_lines(state, da_creare).await;
    }

    Ok(())
}

/// Una riga per cui va creata la scheda prodotto.
struct LineToEnrich {
    id: Uuid,
    description: String,
    ean: Option<String>,
    unit_price: Option<Decimal>,
}

/// Decide cosa fare di ogni riga della fattura, e restituisce quelle da creare.
///
/// # Le regole
///
/// | Riga | Azione | Perché |
/// |---|---|---|
/// | spedizione o sconto | `skip` | Non è un prodotto, non c'è niente da fare |
/// | non classificabile | `needs_review` | Meglio un umano che un'ipotesi |
/// | prodotto senza EAN | `needs_review` | Senza EAN non sappiamo se è già a catalogo |
/// | EAN già a catalogo | `skip` | Esiste già; la giacenza non è di nostra competenza |
/// | EAN a catalogo ma eliminato | `needs_review` | Qualcuno l'aveva tolto: ricrearlo in automatico ignorerebbe quella decisione |
/// | EAN nuovo | `create` | Qui lavora l'agente |
async fn match_lines(db: &PgPool, invoice_id: Uuid) -> Result<Vec<LineToEnrich>, sqlx::Error> {
    let lines = sqlx::query!(
        r#"
        SELECT id, description, raw_text, ean, unit_price, kind as "kind: LineItemKind"
        FROM invoice_line_items
        WHERE invoice_id = $1
        ORDER BY line_no
        "#,
        invoice_id
    )
    .fetch_all(db)
    .await?;

    let mut da_creare = Vec::new();

    for line in lines {
        // `motivo` accompagna le righe che finiscono in revisione: senza, chi
        // apre la pagina vede "needs_review" e non sa perché.
        let (action, status, matched, motivo) = match line.kind {
            LineItemKind::Shipping | LineItemKind::Discount => {
                (LineItemAction::Skip, LineItemStatus::Done, None, None)
            }
            LineItemKind::Unknown => (
                LineItemAction::NeedsReview,
                LineItemStatus::Matched,
                None,
                Some("riga non classificabile come prodotto".to_string()),
            ),
            LineItemKind::Product => match line.ean.as_deref() {
                None => (
                    LineItemAction::NeedsReview,
                    LineItemStatus::Matched,
                    None,
                    Some("EAN non presente in fattura".to_string()),
                ),
                Some(ean) => {
                    let existing = sqlx::query!(
                        r#"SELECT id, status as "status: ProductStatus" FROM products WHERE ean = $1"#,
                        ean
                    )
                    .fetch_optional(db)
                    .await?;

                    match existing {
                        Some(p) if p.status == ProductStatus::Deleted => (
                            LineItemAction::NeedsReview,
                            LineItemStatus::Matched,
                            Some(p.id),
                            Some("prodotto già a catalogo ma eliminato".to_string()),
                        ),
                        Some(p) => (LineItemAction::Skip, LineItemStatus::Done, Some(p.id), None),
                        None => (LineItemAction::Create, LineItemStatus::Matched, None, None),
                    }
                }
            },
        };

        sqlx::query!(
            r#"
            UPDATE invoice_line_items
            SET action = $2, status = $3, matched_product_id = $4, error_message = $5
            WHERE id = $1
            "#,
            line.id,
            action as LineItemAction,
            status as LineItemStatus,
            matched,
            motivo,
        )
        .execute(db)
        .await?;

        if action == LineItemAction::Create {
            da_creare.push(LineToEnrich {
                id: line.id,
                // La descrizione pulita se c'è, altrimenti il testo grezzo:
                // meglio partire da qualcosa di sporco che non partire.
                description: line.description.unwrap_or(line.raw_text),
                ean: line.ean,
                unit_price: line.unit_price,
            });
        }
    }

    Ok(da_creare)
}

/// Genera le schede e le scrive come bozze.
///
/// Non restituisce errore: una riga che fallisce non deve far fallire le altre
/// 39, né l'intera fattura. Ogni riga porta il proprio esito.
async fn enrich_lines(state: &AppState, lines: Vec<LineToEnrich>) {
    // `buffer_unordered` è il pezzo interessante: costruisce uno stream di
    // future e ne tiene N in volo contemporaneamente, avviandone una nuova
    // appena una finisce. Non è un pool di thread — sono task asincroni sullo
    // stesso runtime, in attesa di rete quasi tutto il tempo.
    let esiti: Vec<_> = futures::stream::iter(lines)
        .map(|line| {
            // L'`Arc` si clona per ogni future: costa un contatore, e permette
            // a ciascuna di possedere il proprio riferimento al client.
            let claude = state.claude.clone();
            async move {
                let esito = claude
                    .enrich_product(&line.description, line.ean.as_deref())
                    .await;
                (line, esito)
            }
        })
        .buffer_unordered(ENRICH_CONCURRENCY)
        .collect()
        .await;

    let (mut creati, mut falliti) = (0, 0);

    for (line, esito) in esiti {
        match esito {
            Ok((product, _usage)) => match create_product(&state.db, &line, &product).await {
                Ok(()) => creati += 1,
                Err(err) => {
                    tracing::error!(line = %line.id, error = %err, "scrittura della scheda fallita");
                    mark_line_failed(&state.db, line.id, &err.to_string()).await;
                    falliti += 1;
                }
            },
            Err(err) => {
                tracing::warn!(line = %line.id, error = %err, "arricchimento fallito");
                mark_line_failed(&state.db, line.id, &err.to_string()).await;
                falliti += 1;
            }
        }
    }

    tracing::info!(creati, falliti, "schede prodotto generate");
}

/// Scrive la scheda come bozza e collega la riga di fattura.
async fn create_product(
    db: &PgPool,
    line: &LineToEnrich,
    product: &EnrichedProduct,
) -> Result<(), sqlx::Error> {
    let mut tx = db.begin().await?;

    // `price` resta NULL di proposito: la fattura dà il costo d'acquisto, non
    // il prezzo di vendita. Serve una regola di ricarico, e non la inventiamo.
    let created = sqlx::query_scalar!(
        r#"
        INSERT INTO products
            (ean, title, description, summary, meta_title, meta_description, slug, brand,
             attributes, categories, unit_cost, status, source_line_item_id)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, 'draft', $12)
        ON CONFLICT (ean) DO NOTHING
        RETURNING id
        "#,
        line.ean,
        product.title,
        product.description_html,
        product.summary,
        product.meta_title,
        product.meta_description,
        product.slug,
        product.brand,
        product.attributes(),
        &product.categories(),
        line.unit_price,
        line.id,
    )
    .fetch_optional(&mut *tx)
    .await?;

    // Se lo stesso EAN è comparso due volte, la riga non è più da creare ma da
    // saltare: è il database a dircelo, non lo indoviniamo noi.
    let (action, product_id) = match created {
        Some(id) => (LineItemAction::Create, Some(id)),
        None => {
            let existing = sqlx::query_scalar!("SELECT id FROM products WHERE ean = $1", line.ean)
                .fetch_optional(&mut *tx)
                .await?;
            (LineItemAction::Skip, existing)
        }
    };

    sqlx::query!(
        r#"
        UPDATE invoice_line_items
        SET status = 'done', action = $2, matched_product_id = $3, error_message = NULL
        WHERE id = $1
        "#,
        line.id,
        action as LineItemAction,
        product_id,
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}

async fn mark_line_failed(db: &PgPool, line_id: Uuid, message: &str) {
    if let Err(err) = sqlx::query!(
        "UPDATE invoice_line_items SET status = 'failed', error_message = $2 WHERE id = $1",
        line_id,
        message,
    )
    .execute(db)
    .await
    {
        tracing::error!(line = %line_id, error = %err, "impossibile registrare l'esito della riga");
    }
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

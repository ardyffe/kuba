//! Il client per l'API di Claude.
//!
//! Non esiste un SDK Anthropic ufficiale per Rust, quindi parliamo direttamente
//! con `POST /v1/messages` via reqwest. È meno comodo di un SDK ma non è magia:
//! una richiesta JSON, una risposta JSON, e serde in mezzo.
//!
//! # Come chiediamo l'estrazione
//!
//! Due meccanismi dell'API fanno il lavoro pesante:
//!
//! 1. **Il PDF va nel messaggio.** Un content block di tipo `document` con il
//!    file in base64: non serve estrarre il testo noi. È decisivo qui, perché
//!    l'estrazione testuale delle fatture esce a colonne scombinate (un valore
//!    per riga), mentre il modello vede la pagina come è impaginata.
//!
//! 2. **`output_config.format` vincola la risposta a uno schema JSON.** Non è
//!    "chiedi gentilmente di rispondere in JSON e incrocia le dita": la
//!    risposta *non può* uscire da quello schema. Questo elimina in un colpo
//!    tutta la categoria di errori "il modello ha aggiunto una frase di
//!    cortesia prima del JSON e il parsing è esploso".

use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// L'endpoint dell'API.
const API_URL: &str = "https://api.anthropic.com/v1/messages";

/// La versione dell'API, obbligatoria in ogni richiesta.
const API_VERSION: &str = "2023-06-01";

/// Tetto ai token della risposta. Una fattura da 40 righe sta ampiamente sotto;
/// il limite serve a non pagare una risposta impazzita.
const MAX_TOKENS: u32 = 16000;

/// Timeout della singola chiamata.
///
/// Generoso di proposito: il modello ragiona prima di rispondere e su un PDF di
/// due pagine può metterci un minuto o più. Meglio aspettare che fallire e
/// rifare la chiamata da capo — che costerebbe il doppio.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, thiserror::Error)]
pub enum ClaudeError {
    #[error("errore di rete verso l'API: {0}")]
    Transport(#[from] reqwest::Error),

    /// L'API ha risposto con uno status di errore. Teniamo separati status e
    /// corpo perché il primo dice *come* comportarsi (429 e 5xx si riprovano,
    /// 400 e 401 no) e il secondo dice *cosa* è andato storto.
    #[error("l'API ha risposto {status}: {body}")]
    Api { status: u16, body: String },

    #[error("risposta dell'API non interpretabile: {0}")]
    MalformedResponse(String),

    #[error("il modello ha rifiutato la richiesta ({0})")]
    Refusal(String),
}

impl ClaudeError {
    /// Ha senso riprovare questo errore?
    ///
    /// Una chiave sbagliata (401) o una richiesta malformata (400) falliranno
    /// identicamente al secondo tentativo: riprovare brucia solo tempo e soldi.
    /// Un 429 o un 503 invece sono transitori per definizione.
    pub fn is_retryable(&self) -> bool {
        match self {
            ClaudeError::Transport(err) => err.is_timeout() || err.is_connect(),
            ClaudeError::Api { status, .. } => *status == 429 || *status >= 500,
            ClaudeError::MalformedResponse(_) => true,
            ClaudeError::Refusal(_) => false,
        }
    }
}

/// Il client. Contiene una `reqwest::Client`, che va **riusata**: dentro tiene
/// il pool di connessioni HTTP, e ricrearla a ogni chiamata rifarebbe ogni
/// volta l'handshake TLS.
pub struct ClaudeClient {
    http: reqwest::Client,
    api_key: String,
    model: String,
}

impl ClaudeClient {
    pub fn new(api_key: String, model: String) -> Result<Self, reqwest::Error> {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()?;

        Ok(Self {
            http,
            api_key,
            model,
        })
    }

    /// Manda il PDF al modello e riceve la fattura strutturata.
    pub async fn extract_invoice(&self, pdf: &[u8]) -> Result<ExtractedInvoice, ClaudeError> {
        // Il PDF viaggia in base64 dentro il JSON, senza a capo.
        let encoded = BASE64.encode(pdf);

        let body = json!({
            "model": self.model,
            "max_tokens": MAX_TOKENS,
            "system": SYSTEM_PROMPT,
            "messages": [{
                "role": "user",
                "content": [
                    // Il documento va **prima** del testo: il modello legge in
                    // ordine, e l'istruzione ha senso solo dopo aver visto il file.
                    {
                        "type": "document",
                        "source": {
                            "type": "base64",
                            "media_type": "application/pdf",
                            "data": encoded
                        }
                    },
                    { "type": "text", "text": USER_PROMPT }
                ]
            }],
            "output_config": {
                "format": {
                    "type": "json_schema",
                    "schema": extraction_schema()
                }
            }
        });

        let response = self
            .http
            .post(API_URL)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION)
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        let text = response.text().await?;

        if !status.is_success() {
            return Err(ClaudeError::Api {
                status: status.as_u16(),
                // Tronchiamo: un corpo d'errore enorme finirebbe nei log e
                // nella colonna error_message senza aggiungere informazione.
                body: text.chars().take(500).collect(),
            });
        }

        let message: ApiResponse = serde_json::from_str(&text)
            .map_err(|err| ClaudeError::MalformedResponse(err.to_string()))?;

        if message.stop_reason.as_deref() == Some("refusal") {
            return Err(ClaudeError::Refusal(
                message.stop_reason.unwrap_or_default(),
            ));
        }

        // Con il ragionamento attivo la risposta può contenere più blocchi:
        // prendiamo il primo di tipo `text`, che grazie a `output_config`
        // contiene JSON valido conforme allo schema.
        let payload = message
            .content
            .iter()
            .find_map(|block| match block {
                ContentBlock::Text { text } => Some(text.as_str()),
                ContentBlock::Other => None,
            })
            .ok_or_else(|| {
                ClaudeError::MalformedResponse("nessun blocco di testo nella risposta".to_string())
            })?;

        let extracted: ExtractedInvoice = serde_json::from_str(payload).map_err(|err| {
            ClaudeError::MalformedResponse(format!("JSON non conforme allo schema: {err}"))
        })?;

        tracing::info!(
            righe = extracted.lines.len(),
            input_token = message.usage.input_tokens,
            output_token = message.usage.output_tokens,
            "estrazione completata"
        );

        Ok(extracted)
    }
}

// ---------------------------------------------------------------------------
// I prompt
// ---------------------------------------------------------------------------

const SYSTEM_PROMPT: &str = "\
Sei un assistente che estrae dati da fatture di fornitori di profumeria.
Il tuo unico compito è trascrivere ciò che è scritto sulla fattura, con precisione assoluta.

Regole inderogabili:
- Non inventare mai un dato. Se un valore non è presente sulla fattura, usa null.
- In particolare non dedurre mai un codice EAN/GTIN da altri prodotti o dalla tua conoscenza:
  o è stampato sulla fattura, o è null.
- Trascrivi tutte le righe della tabella, comprese quelle che non sono prodotti.
- Classifica ogni riga: 'product' per un articolo, 'shipping' per spese di spedizione,
  'discount' per sconti o abbuoni, 'unknown' se non riesci a decidere.
- Gli importi vanno come stringhe con il punto decimale e senza simbolo di valuta:
  '8,10 €' diventa '8.10'.
- Le date in formato ISO: 13.03.2026 diventa '2026-03-13'.
- La valuta come codice ISO a tre lettere: 'EUR'.";

const USER_PROMPT: &str = "\
Estrai i dati di testata e tutte le righe di questa fattura.

Per ogni riga:
- `line_no`: la posizione nella tabella, partendo da 1, nell'ordine in cui appare.
- `raw_text`: il testo della riga così come è stampato, per intero. Serve a un umano
  per verificare l'estrazione, quindi deve essere fedele all'originale.
- `description`: il nome del prodotto ripulito dalla formattazione.
- `ean`: il codice EAN/GTIN se presente (di solito 13 cifre), altrimenti null.
- `supplier_sku`: il codice articolo interno del fornitore, se diverso dall'EAN.
- `quantity`, `unit_price`, `amount`: come stampati.
- `kind`: la classificazione della riga.";

/// Lo schema JSON che vincola la risposta.
///
/// Nota due dettagli:
///
/// - **Gli importi sono `string`, non `number`.** I numeri JSON sono in virgola
///   mobile: `8.10` diventerebbe `8.099999...`. Li riceviamo come testo e li
///   convertiamo in `Decimal`, esattamente come facciamo con il database.
/// - **I campi opzionali usano `["string", "null"]`**, e *tutti* i campi sono in
///   `required`. Sembra contraddittorio ma non lo è: "obbligatorio" qui
///   significa "devi pronunciarti", e pronunciarsi può voler dire `null`. È il
///   modo di impedire che un campo sparisca silenziosamente dalla risposta.
fn extraction_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["supplier_name", "invoice_number", "invoice_date", "currency", "total_amount", "lines"],
        "properties": {
            "supplier_name":  { "type": ["string", "null"] },
            "invoice_number": { "type": ["string", "null"] },
            "invoice_date":   { "type": ["string", "null"], "description": "formato ISO YYYY-MM-DD" },
            "currency":       { "type": ["string", "null"], "description": "codice ISO a 3 lettere" },
            "total_amount":   { "type": ["string", "null"] },
            "lines": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["line_no", "raw_text", "description", "ean", "supplier_sku", "quantity", "unit_price", "amount", "kind"],
                    "properties": {
                        "line_no":      { "type": "integer" },
                        "raw_text":     { "type": "string" },
                        "description":  { "type": ["string", "null"] },
                        "ean":          { "type": ["string", "null"] },
                        "supplier_sku": { "type": ["string", "null"] },
                        "quantity":     { "type": ["integer", "null"] },
                        "unit_price":   { "type": ["string", "null"] },
                        "amount":       { "type": ["string", "null"] },
                        "kind": {
                            "type": "string",
                            "enum": ["product", "shipping", "discount", "unknown"]
                        }
                    }
                }
            }
        }
    })
}

// ---------------------------------------------------------------------------
// I tipi della risposta
// ---------------------------------------------------------------------------

/// La risposta dell'API, di cui ci interessano solo tre campi.
///
/// serde ignora per default i campi che non dichiariamo: la risposta ne
/// contiene molti altri (`id`, `role`, `model`...) e va benissimo così.
#[derive(Debug, Deserialize)]
struct ApiResponse {
    content: Vec<ContentBlock>,
    stop_reason: Option<String>,
    usage: Usage,
}

/// Un blocco di contenuto.
///
/// `#[serde(tag = "type")]` è il modo di rappresentare in Rust un JSON
/// "polimorfo": serde legge il campo `type` e sceglie la variante. Tutti i tipi
/// che non ci servono (ragionamento, uso di strumenti) finiscono in `Other`
/// grazie a `#[serde(other)]`, invece di far fallire la deserializzazione.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ContentBlock {
    Text {
        text: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct Usage {
    input_tokens: u32,
    output_tokens: u32,
}

// ---------------------------------------------------------------------------
// Il risultato dell'estrazione
// ---------------------------------------------------------------------------

/// La fattura come l'ha letta il modello.
///
/// Tutti i campi sono `Option` e le stringhe non sono ancora convertite: qui
/// rappresentiamo *quello che è arrivato*, non *quello che ci serve*. La
/// conversione (date, decimali) avviene dopo, dove possiamo gestire un valore
/// malformato senza far fallire l'intera estrazione.
#[derive(Debug, Deserialize, Serialize)]
pub struct ExtractedInvoice {
    pub supplier_name: Option<String>,
    pub invoice_number: Option<String>,
    pub invoice_date: Option<String>,
    pub currency: Option<String>,
    pub total_amount: Option<String>,
    pub lines: Vec<ExtractedLine>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ExtractedLine {
    pub line_no: i32,
    pub raw_text: String,
    pub description: Option<String>,
    pub ean: Option<String>,
    pub supplier_sku: Option<String>,
    pub quantity: Option<i32>,
    pub unit_price: Option<String>,
    pub amount: Option<String>,
    pub kind: String,
}

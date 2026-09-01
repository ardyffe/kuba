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
use serde::de::DeserializeOwned;
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

/// Quante ricerche web al massimo per ogni prodotto.
///
/// E un tetto di spesa prima ancora che di tempo: ogni ricerca costa $10 ogni
/// 1.000, e i risultati rientrano come token di input.
const MAX_SEARCHES: u32 = 4;

/// Quante volte al massimo riprendiamo un turno interrotto con pause_turn.
/// Serve a impedire un ciclo infinito se qualcosa va storto lato API.
const MAX_TURNS: usize = 5;

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
    /// Modello per l'estrazione: e' trascrizione, un modello piccolo basta.
    model: String,
    /// Modello per l'arricchimento: qui si scrive il testo che finira' sul
    /// sito del cliente, e la qualita' conta di piu'. Separato apposta, per
    /// poterlo alzare senza rendere piu' cara anche l'estrazione.
    enrichment_model: String,
    /// Modello per la rilettura linguistica, se attiva.
    ///
    /// La divisione dei compiti nasce da una misura, non da un principio:
    /// Haiku cerca bene e struttura bene, sbaglia solo l'italiano. Usare un
    /// modello grande per l'intera generazione costa 5,6 volte tanto, perche'
    /// il grosso della spesa sono i risultati di ricerca che entrano nel
    /// contesto. Usarlo per rileggere ~1.200 token di testo costa quasi niente.
    proofread_model: Option<String>,
}

impl ClaudeClient {
    pub fn new(
        api_key: String,
        model: String,
        enrichment_model: String,
        proofread_model: Option<String>,
    ) -> Result<Self, reqwest::Error> {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()?;

        Ok(Self {
            http,
            api_key,
            model,
            enrichment_model,
            proofread_model,
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

        let message = self.send(body).await?;
        let extracted: ExtractedInvoice = json_from_blocks(&message.content)?;

        tracing::info!(
            righe = extracted.lines.len(),
            input_token = message.usage.input_tokens,
            output_token = message.usage.output_tokens,
            "estrazione completata"
        );

        Ok(extracted)
    }

    /// `POST /v1/messages`, con la ripresa dei turni interrotti.
    ///
    /// # `pause_turn`
    ///
    /// Quando il modello usa la ricerca web, l'API puo' interrompere un turno
    /// lungo e rispondere `stop_reason: "pause_turn"`. Non e' un errore: e' un
    /// "continua". Si riprende rimandando indietro il messaggio dell'assistente
    /// **identico a come e' arrivato** — i risultati di ricerca contengono un
    /// campo cifrato che l'API deve poter decifrare, e alterarlo fa fallire la
    /// richiesta con un 400.
    ///
    /// E' il motivo per cui teniamo i blocchi come `serde_json::Value` grezzi
    /// invece di deserializzarli in tipi nostri: per rimandarli indietro
    /// intatti dobbiamo poterli riprodurre byte per byte.
    async fn send(&self, mut body: serde_json::Value) -> Result<ApiResponse, ClaudeError> {
        for _ in 0..MAX_TURNS {
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

            match message.stop_reason.as_deref() {
                // Un rifiuto arriva con HTTP 200: senza questo controllo
                // cercheremmo del JSON in una risposta che non ne contiene.
                Some("refusal") => {
                    return Err(ClaudeError::Refusal(
                        "il modello ha declinato la richiesta".to_string(),
                    ));
                }
                Some("pause_turn") => {
                    let messages = body["messages"].as_array_mut().ok_or_else(|| {
                        ClaudeError::MalformedResponse("richiesta senza messaggi".to_string())
                    })?;
                    messages.push(json!({ "role": "assistant", "content": message.content }));
                    continue;
                }
                _ => return Ok(message),
            }
        }

        Err(ClaudeError::MalformedResponse(
            "troppe riprese consecutive del turno".to_string(),
        ))
    }

    /// Cerca online le informazioni di un profumo e ne compone la scheda.
    ///
    /// Riceve quello che sappiamo dalla fattura — descrizione grezza ed EAN —
    /// e restituisce la scheda completa in italiano.
    pub async fn enrich_product(
        &self,
        raw_description: &str,
        ean: Option<&str>,
    ) -> Result<(EnrichedProduct, Usage), ClaudeError> {
        let ean_line = match ean {
            Some(code) => format!("EAN/GTIN: {code}"),
            None => "EAN/GTIN: non disponibile".to_string(),
        };

        let body = json!({
            "model": self.enrichment_model,
            "max_tokens": MAX_TOKENS,
            "system": ENRICHMENT_SYSTEM_PROMPT,
            "messages": [{
                "role": "user",
                "content": format!(
                    "Prodotto da schedare, come appare sulla fattura del fornitore:

                     Descrizione: {raw_description}
{ean_line}

                     Cerca online le informazioni su questo profumo e componi la scheda."
                )
            }],
            // La ricerca web e' un tool *server-side*: la esegue l'API, non noi.
            // Non c'e' nessun ciclo di tool use da gestire, arrivano i risultati.
            "tools": [{
                "type": "web_search_20250305",
                "name": "web_search",
                "max_uses": MAX_SEARCHES
            }],
            "output_config": {
                "format": {
                    "type": "json_schema",
                    "schema": enrichment_schema()
                }
            }
        });

        let message = self.send(body).await?;
        let mut product: EnrichedProduct = json_from_blocks(&message.content)?;

        // Nessuna ricerca significa che ha scritto a memoria: qualunque cosa
        // dichiari, per noi la confidenza e' bassa.
        //
        // Il prompt puo' chiedere di cercare, ma solo il contatore delle
        // ricerche lo dimostra. E' la differenza fra una speranza e una
        // garanzia: la seconda sta nel codice.
        if message.usage.searches() == 0 {
            let dettaglio = product
                .note_revisione
                .take()
                .unwrap_or_else(|| "Verificare le note olfattive.".to_string());

            product.confidenza = "bassa".to_string();
            product.note_revisione = Some(format!(
                "Scheda scritta senza consultare fonti online. {dettaglio}"
            ));

            tracing::warn!(
                titolo = product.title,
                "scheda generata senza ricerche: confidenza forzata a bassa"
            );
        }

        tracing::info!(
            titolo = product.title,
            confidenza = product.confidenza,
            ricerche = message.usage.searches(),
            input_token = message.usage.input_tokens,
            output_token = message.usage.output_tokens,
            "scheda generata"
        );

        Ok((product, message.usage))
    }
}

/// Corregge la lingua dei testi di una scheda, senza toccarne il contenuto.
///
/// Non fa ricerche e non vede i dati del prodotto: riceve solo il testo. E'
/// questo che la rende quasi gratis anche con un modello grande — il costo
/// dell'arricchimento sta nei risultati di ricerca, non nel modello.
///
/// Se `ANTHROPIC_PROOFREAD_MODEL` non e' impostata, non fa niente.
impl ClaudeClient {
    pub async fn proofread(
        &self,
        product: &mut EnrichedProduct,
    ) -> Result<Option<Usage>, ClaudeError> {
        let Some(model) = &self.proofread_model else {
            return Ok(None);
        };

        let testi = json!({
            "title": product.title,
            "description_html": product.description_html,
            "summary": product.summary,
            "meta_title": product.meta_title,
            "meta_description": product.meta_description,
        });

        let body = json!({
            "model": model,
            "max_tokens": MAX_TOKENS,
            "system": PROOFREAD_SYSTEM_PROMPT,
            "messages": [{
                "role": "user",
                "content": format!(
                    "Correggi la lingua di questi testi e restituiscili nello stesso formato:\n\n{testi}"
                )
            }],
            "output_config": {
                "format": { "type": "json_schema", "schema": proofread_schema() }
            }
        });

        let message = self.send(body).await?;
        let corretti: ProofreadTexts = json_from_blocks(&message.content)?;

        product.title = corretti.title;
        product.description_html = corretti.description_html;
        product.summary = corretti.summary;
        product.meta_title = corretti.meta_title;
        product.meta_description = corretti.meta_description;

        tracing::info!(
            input_token = message.usage.input_tokens,
            output_token = message.usage.output_tokens,
            "testi riletti"
        );

        Ok(Some(message.usage))
    }
}

const PROOFREAD_SYSTEM_PROMPT: &str = "\
Sei un revisore di italiano per schede prodotto di un ecommerce di profumi.

Correggi **solo** la lingua:
- refusi ed errori di ortografia (per esempio patciuli al posto di patchouli);
- errori di grammatica, concordanza e costruzione;
- parole di altre lingue lasciate per sbaglio nel testo italiano, sostituendole con
  l equivalente italiano corrente. I nomi propri di profumi e marchi restano invariati,
  cosi come i termini tecnici della profumeria che in italiano si usano tali e quali
  (eau de parfum, extrait, sillage).

Non fare nient altro:
- non riscrivere le frasi che sono gia corrette;
- non abbellire, non accorciare, non allungare;
- non cambiare nessun dato di fatto: note olfattive, anni, nomi, formati restano come
  sono, anche se ti sembrano sbagliati.

Se un testo e gia corretto, restituiscilo identico.";

fn proofread_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["title", "description_html", "summary", "meta_title", "meta_description"],
        "properties": {
            "title":            { "type": "string" },
            "description_html": { "type": "string" },
            "summary":          { "type": ["string", "null"] },
            "meta_title":       { "type": ["string", "null"] },
            "meta_description": { "type": ["string", "null"] }
        }
    })
}

#[derive(Debug, Deserialize)]
struct ProofreadTexts {
    title: String,
    description_html: String,
    summary: Option<String>,
    meta_title: Option<String>,
    meta_description: Option<String>,
}

/// Cerca il JSON conforme allo schema fra i blocchi della risposta.
///
/// Si scorre **dall'ultimo al primo**, e non e' un dettaglio: con la ricerca
/// web attiva i primi blocchi di testo sono il ragionamento a voce alta del
/// modello ("cerco le note olfattive di..."), mentre la risposta strutturata e'
/// l'ultima cosa che scrive. Prendere il primo blocco funzionava per
/// l'estrazione, dove ce n'e' uno solo, e si sarebbe rotto qui.
fn json_from_blocks<T: DeserializeOwned>(content: &[serde_json::Value]) -> Result<T, ClaudeError> {
    for block in content.iter().rev() {
        if block.get("type").and_then(serde_json::Value::as_str) != Some("text") {
            continue;
        }
        let Some(text) = block.get("text").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if let Ok(value) = serde_json::from_str::<T>(text.trim()) {
            return Ok(value);
        }
    }

    Err(ClaudeError::MalformedResponse(
        "nessun blocco di testo contiene JSON conforme allo schema".to_string(),
    ))
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
    /// Blocchi grezzi: vedi il commento su `send` per il perche'.
    content: Vec<serde_json::Value>,
    stop_reason: Option<String>,
    usage: Usage,
}

#[derive(Debug, Deserialize)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    /// Presente solo quando sono stati usati tool server-side.
    #[serde(default)]
    server_tool_use: Option<ServerToolUse>,
}

impl Usage {
    /// Quante ricerche web sono state fatte davvero: e' la voce che si paga
    /// a parte ($10 ogni 1.000), quindi vogliamo vederla nei log.
    pub fn searches(&self) -> u32 {
        self.server_tool_use
            .as_ref()
            .map_or(0, |usage| usage.web_search_requests)
    }
}

#[derive(Debug, Deserialize)]
struct ServerToolUse {
    #[serde(default)]
    web_search_requests: u32,
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

// ---------------------------------------------------------------------------
// Arricchimento: prompt, schema e tipi
// ---------------------------------------------------------------------------

const ENRICHMENT_SYSTEM_PROMPT: &str = "\
Componi schede prodotto per un ecommerce italiano di profumi orientali.

Il metodo è sempre lo stesso: prima **cerca online** il prodotto specifico, poi scrivi
usando solo quello che hai trovato. Fai sempre almeno una ricerca, anche quando credi di
conoscere già il prodotto: la memoria confonde le varianti con nomi simili, e una scheda
scritta senza fonti viene scartata.

Regole inderogabili:
- Non inventare mai le note olfattive. Se non le trovi su fonti attendibili, lascia gli
  array vuoti e metti confidenza 'bassa'. Una scheda incompleta si corregge; una scheda
  con note inventate finisce online e nessuno se ne accorge.
- Verifica di aver trovato **quel** prodotto e non un omonimo: i brand orientali hanno
  molte varianti con nomi simili (Intense, Elixir, Extreme, Pride...). Nel dubbio,
  confidenza 'bassa'.
- Non tutto è un profumo: distingui eau de parfum, eau de toilette, body mist, deodorante,
  extrait. Un body mist non ha 'note di fondo' come un EDP, e va scritto per quello che è.
- Scrivi in italiano semplice e concreto. Frasi brevi, parole comuni. Descrivi il profumo,
  non l'uomo o la donna che lo indossa: niente prosa evocativa, niente metafore, niente
  superlativi che non puoi sostenere.
- Rileggi il testo prima di consegnarlo: deve essere italiano corretto, senza refusi e
  senza parole di altre lingue.
- Le note olfattive vanno in minuscolo e al singolare quando ha senso: bergamotto,
  fava tonka, legno di sandalo.
- Il campo `fonti` deve contenere gli URL che hai davvero consultato.

Criteri di confidenza:
- 'alta': hai trovato il prodotto su fonti concordanti, con note olfattive esplicite.
- 'media': hai trovato il prodotto ma alcune informazioni mancano o le fonti divergono.
- 'bassa': non l'hai trovato, o non sei sicuro che sia lo stesso prodotto.

Alcuni campi sono vincolati a una lista chiusa (durata, scia, famiglia olfattiva, genere,
prodotto): scegli sempre il valore della lista più vicino, mai una formulazione tua.

Formato dei testi:
- description_html: 4-6 paragrafi brevi in tag <p>: apertura, descrizione olfattiva,
  evoluzione delle note, occasioni d'uso e destinatario.
- summary: un solo <p> di una frase.
- meta_title: massimo 70 caratteri.
- meta_description: massimo 155 caratteri.
- slug: minuscolo, parole separate da trattini, senza accenti.";

/// Lo schema della scheda prodotto.
///
/// Ricalca i campi del backend del cliente, feature comprese. I nomi in
/// italiano non sono un vezzo: sono le stesse etichette che l'operatore vede
/// nella sua interfaccia, e finiscono tali e quali nella colonna `attributes`.
fn enrichment_schema() -> serde_json::Value {
    let lista = json!({ "type": "array", "items": { "type": "string" } });

    // I vocabolari chiusi.
    //
    // Nel backend del cliente questi campi sono menu a tendina, non testo
    // libero: se il modello scrive "8-10 ore" dove l'interfaccia si aspetta
    // "eccellente", l'import crea un attributo nuovo invece di usare quello
    // esistente. Vincolarli qui significa che la risposta *non può* uscire
    // dalla lista — lo stesso meccanismo che sull'estrazione dà il 100%.
    //
    // ATTENZIONE: questi elenchi sono ricostruiti dallo screenshot del backend
    // ("eccellente" per la durata, "intensa" per la scia, "gourmand" e
    // "orientali" per la famiglia) e completati per analogia. Vanno riconciliati
    // con i menu veri: cambiare un elenco è una riga.
    let durata = json!({
        "type": "string",
        "enum": ["scarsa", "discreta", "buona", "ottima", "eccellente"]
    });
    let scia = json!({
        "type": "string",
        "enum": ["intima", "moderata", "intensa", "enorme"]
    });
    let famiglia = json!({
        "type": "array",
        "items": {
            "type": "string",
            "enum": [
                "orientali", "gourmand", "legnosi", "floreali", "fruttati", "speziati",
                "agrumati", "aromatici", "acquatici", "ambrati", "cipriati", "muschiati"
            ]
        }
    });
    let prodotto = json!({
        "type": "string",
        "enum": [
            "eau de parfum", "eau de toilette", "extrait de parfum",
            "body mist", "deodorante", "acqua profumata"
        ]
    });

    json!({
        "type": "object",
        "additionalProperties": false,
        "required": [
            "title", "brand", "description_html", "summary", "meta_title", "meta_description",
            "slug", "note_di_testa", "note_di_cuore", "note_di_fondo", "famiglia_olfattiva",
            "genere", "ml", "prodotto", "durata", "scia", "questo_profumo_ricorda",
            "piace_anche", "confidenza", "fonti", "note_revisione"
        ],
        "properties": {
            "title":            { "type": "string" },
            "brand":            { "type": ["string", "null"] },
            "description_html": { "type": "string" },
            "summary":          { "type": ["string", "null"] },
            "meta_title":       { "type": ["string", "null"] },
            "meta_description": { "type": ["string", "null"] },
            "slug":             { "type": ["string", "null"] },
            "note_di_testa":      lista,
            "note_di_cuore":      lista,
            "note_di_fondo":      lista,
            "famiglia_olfattiva": famiglia,
            // Un enum non si può combinare con un tipo nullabile: l'API rifiuta
            // lo schema con un 400. Invece di rinunciare al vincolo, il "non so"
            // diventa un valore esplicito della lista. Stessa cosa per durata,
            // scia e prodotto, che per questo hanno "non specificato".
            "genere":   { "type": "string",
                          "enum": ["uomo", "donna", "unisex", "non specificato"] },
            "ml":       { "type": ["integer", "null"] },
            "prodotto": prodotto,
            "durata":   durata,
            "scia":     scia,
            "questo_profumo_ricorda": lista,
            "piace_anche":            lista,
            // `categorie` non c'è più: le calcoliamo noi da genere, famiglia e
            // brand. Vedi `EnrichedProduct::categories`.
            "confidenza": { "type": "string", "enum": ["alta", "media", "bassa"] },
            "fonti":      lista,
            "note_revisione": { "type": ["string", "null"],
                                "description": "dubbi o avvertenze per chi rivede la scheda" }
        }
    })
}

/// La scheda prodotto generata.
#[derive(Debug, Deserialize, Serialize)]
pub struct EnrichedProduct {
    pub title: String,
    pub brand: Option<String>,
    pub description_html: String,
    pub summary: Option<String>,
    pub meta_title: Option<String>,
    pub meta_description: Option<String>,
    pub slug: Option<String>,
    pub note_di_testa: Vec<String>,
    pub note_di_cuore: Vec<String>,
    pub note_di_fondo: Vec<String>,
    pub famiglia_olfattiva: Vec<String>,
    pub genere: String,
    pub ml: Option<i32>,
    pub prodotto: String,
    pub durata: String,
    pub scia: String,
    pub questo_profumo_ricorda: Vec<String>,
    pub piace_anche: Vec<String>,
    /// `alta` | `media` | `bassa`. È il controllo qualità più importante di
    /// tutta la pipeline: permette al modello di dire "non l'ho trovato"
    /// invece di produrre una scheda plausibile e falsa.
    pub confidenza: String,
    pub fonti: Vec<String>,
    pub note_revisione: Option<String>,
}

impl EnrichedProduct {
    /// Le categorie dell'ecommerce, **calcolate** invece che chieste al modello.
    ///
    /// Guardando le categorie di un prodotto reale nel backend del cliente —
    /// `Famiglia Olfattiva`, `Profumi Unisex`, `Gulf Orchid`, `Profumi Gourmand`,
    /// `Marchi`, `Home`, `Profumi Orientali` — si vede che non sono una scelta
    /// editoriale: sono una funzione di genere, famiglia olfattiva e brand.
    ///
    /// Se sono derivabili, derivarle è meglio che chiederle: il modello non può
    /// inventare `Fragranze Uomo` dove l'albero ha `Profumi Uomo`, e due schede
    /// dello stesso tipo finiscono sempre nelle stesse categorie.
    pub fn categories(&self) -> Vec<String> {
        // Voci fisse presenti su ogni scheda del cliente.
        let mut categorie = vec![
            "Home".to_string(),
            "Marchi".to_string(),
            "Famiglia Olfattiva".to_string(),
        ];

        if let Some(brand) = &self.brand {
            categorie.push(brand.clone());
        }

        if self.genere != "non specificato" {
            categorie.push(format!("Profumi {}", capitalizza(&self.genere)));
        }

        for famiglia in &self.famiglia_olfattiva {
            categorie.push(format!("Profumi {}", capitalizza(famiglia)));
        }

        categorie
    }

    /// Le feature come finiscono nella colonna `attributes`, con le stesse
    /// etichette che l'operatore vede nel backend.
    pub fn attributes(&self) -> serde_json::Value {
        json!({
            "note_di_testa": self.note_di_testa,
            "note_di_cuore": self.note_di_cuore,
            "note_di_fondo": self.note_di_fondo,
            "famiglia_olfattiva": self.famiglia_olfattiva,
            "genere": self.genere,
            "ml": self.ml,
            "prodotto": self.prodotto,
            "durata": self.durata,
            "scia": self.scia,
            "questo_profumo_ricorda": self.questo_profumo_ricorda,
            "piace_anche": self.piace_anche,
            // I campi con underscore iniziale sono nostri, non del cliente:
            // servono a chi rivede la scheda per sapere quanto fidarsi.
            "_confidenza": self.confidenza,
            "_fonti": self.fonti,
            "_note_revisione": self.note_revisione,
        })
    }
}

/// Prima lettera maiuscola. `chars()` e non un indice: in UTF-8 il primo
/// carattere non occupa sempre un byte solo.
fn capitalizza(parola: &str) -> String {
    let mut caratteri = parola.chars();
    match caratteri.next() {
        Some(primo) => primo.to_uppercase().collect::<String>() + caratteri.as_str(),
        None => String::new(),
    }
}

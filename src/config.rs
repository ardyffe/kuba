//! Lettura e validazione della configurazione dall'ambiente.
//!
//! Regola che seguiamo in tutto il progetto: l'ambiente si legge **una volta
//! sola**, all'avvio. Se manca qualcosa il processo muore subito con un
//! messaggio chiaro, invece di scoprirlo alla prima richiesta HTTP.

use std::env;
use std::path::PathBuf;

/// I parametri di cui il server ha bisogno per partire.
///
/// `Debug` per poterla stampare nei log, `Clone` perché ci servirà copiarla.
#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub port: u16,
    /// Radice dello storage su disco. Sotto ci finiscono i PDF delle fatture.
    pub storage_dir: PathBuf,
    pub anthropic_api_key: String,
    pub anthropic_model: String,
}

/// Gli errori possibili durante il caricamento.
///
/// `thiserror` genera per noi l'implementazione del trait `std::error::Error` e
/// il messaggio di `Display` a partire dall'attributo `#[error(...)]`.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("variabile d'ambiente mancante o vuota: {0}")]
    Missing(&'static str),

    #[error("variabile d'ambiente {name} non valida (valore: {value}): {reason}")]
    Invalid {
        name: &'static str,
        value: String,
        reason: String,
    },
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        Ok(Self {
            database_url: required("DATABASE_URL")?,
            port: optional_port("PORT", 3000)?,
            storage_dir: PathBuf::from(optional("STORAGE_DIR", "storage")),
            // Obbligatoria: da M4 il worker non sa fare il suo mestiere senza.
            // Meglio non partire affatto che partire e fallire ogni fattura.
            anthropic_api_key: required("ANTHROPIC_API_KEY")?,
            // Sovrascrivibile per provare un modello diverso senza ricompilare.
            anthropic_model: optional("ANTHROPIC_MODEL", "claude-haiku-4-5"),
        })
    }

    /// Dove finiscono i PDF delle fatture.
    ///
    /// Restituisce un `PathBuf` (posseduto) e non un `&Path` (prestato) perché
    /// il percorso lo costruiamo qui sul momento: non esiste da nessuna parte
    /// un valore a cui poter fare riferimento.
    pub fn invoices_dir(&self) -> PathBuf {
        self.storage_dir.join("invoices")
    }

    /// La *chiave* con cui il file è registrato nel database: `invoices/{id}.pdf`.
    ///
    /// È una chiave logica, non un percorso di sistema: sempre con `/`, sempre
    /// relativa alla radice dello storage. Così il contenuto del database resta
    /// valido se cambiamo `STORAGE_DIR`, se spostiamo il servizio su Linux o se
    /// un domani passiamo a un object storage, dove questa diventerebbe la key.
    ///
    /// Il nome del file è l'UUID: mai il nome originale caricato dall'utente,
    /// che potrebbe contenere `../` o caratteri validi solo su alcuni sistemi.
    pub fn invoice_key(id: &uuid::Uuid) -> String {
        format!("invoices/{id}.pdf")
    }

    /// Traduce una chiave logica nel percorso reale su questo sistema.
    ///
    /// `Path::join` gestisce da sé i separatori, quindi `invoices/x.pdf` su
    /// Windows diventa un percorso valido senza conversioni manuali.
    pub fn resolve(&self, key: &str) -> PathBuf {
        self.storage_dir.join(key)
    }
}

fn required(name: &'static str) -> Result<String, ConfigError> {
    // `match` su Result: il caso "presente ma stringa vuota" è comunque un errore,
    // e la guardia `if` ci permette di esprimerlo senza un secondo controllo.
    match env::var(name) {
        Ok(value) if !value.trim().is_empty() => Ok(value),
        _ => Err(ConfigError::Missing(name)),
    }
}

fn optional(name: &'static str, default: &str) -> String {
    match env::var(name) {
        Ok(value) if !value.trim().is_empty() => value,
        _ => default.to_string(),
    }
}

fn optional_port(name: &'static str, default: u16) -> Result<u16, ConfigError> {
    match env::var(name) {
        // Non impostata: usiamo il default, non è un errore.
        Err(_) => Ok(default),
        Ok(raw) => match raw.parse::<u16>() {
            Ok(port) => Ok(port),
            Err(err) => Err(ConfigError::Invalid {
                name,
                value: raw,
                reason: err.to_string(),
            }),
        },
    }
}

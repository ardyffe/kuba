//! Lettura e validazione della configurazione dall'ambiente.
//!
//! Regola che seguiamo in tutto il progetto: l'ambiente si legge **una volta
//! sola**, all'avvio. Se manca qualcosa il processo muore subito con un
//! messaggio chiaro, invece di scoprirlo alla prima richiesta HTTP.

use std::env;

/// I parametri di cui il server ha bisogno per partire.
///
/// `Debug` per poterla stampare nei log, `Clone` perché ci servirà copiarla.
#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub port: u16,
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
        })
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

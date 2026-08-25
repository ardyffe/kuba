//! Connessione al database ed esecuzione delle migration.

use std::time::Duration;

use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

use crate::config::Config;

/// Apre il pool di connessioni verso Postgres.
///
/// Un *pool* tiene aperte N connessioni e le presta agli handler: aprire una
/// connessione TCP + handshake a ogni richiesta costerebbe millisecondi inutili.
pub async fn connect(config: &Config) -> Result<PgPool, sqlx::Error> {
    let pool = PgPoolOptions::new()
        .max_connections(10)
        // Senza questo timeout, se il DB è irraggiungibile la richiesta resta
        // appesa per sempre invece di fallire con un errore leggibile.
        .acquire_timeout(Duration::from_secs(5))
        .connect(&config.database_url)
        .await?;

    tracing::info!("connesso a Postgres");
    Ok(pool)
}

/// Applica le migration presenti in `./migrations`.
///
/// `sqlx::migrate!` è una macro che gira **a compile time**: legge la cartella
/// mentre compili e incorpora gli .sql dentro il binario. Due conseguenze:
/// il binario in produzione non ha bisogno dei file, e se cancelli una
/// migration il progetto non compila più.
///
/// sqlx tiene una tabella `_sqlx_migrations` con il checksum di ogni file
/// applicato: se modifichi una migration già applicata, al riavvio ottieni un
/// errore invece di un DB silenziosamente disallineato.
pub async fn run_migrations(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!("./migrations").run(pool).await?;
    tracing::info!("migration applicate");
    Ok(())
}

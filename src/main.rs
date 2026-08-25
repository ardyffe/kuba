//! kuba — backend che trasforma le fatture dei fornitori in schede prodotto.
//!
//! Questo file fa una cosa sola: mettere in piedi il servizio nell'ordine
//! giusto e poi restare in ascolto.

mod config;
mod db;
mod error;
mod extract;
mod models;
mod routes;
mod state;

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::TcpListener;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::config::Config;
use crate::state::AppState;

/// `#[tokio::main]` trasforma questa `async fn` in una `fn main` normale che
/// avvia il runtime asincrono e ci esegue dentro il nostro codice.
///
/// Il tipo di ritorno `Result<_, Box<dyn Error>>` ci permette di usare `?`
/// anche qui: qualunque errore all'avvio termina il processo stampando il
/// messaggio, che in fase di boot è il comportamento che vogliamo.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Carica il file .env se esiste. `.ok()` scarta l'errore di proposito:
    // in produzione le variabili arrivano dall'ambiente e il file non c'è.
    dotenvy::dotenv().ok();

    init_tracing();

    let config = Config::from_env()?;
    let db = db::connect(&config).await?;
    db::run_migrations(&db).await?;

    // La cartella dei PDF viene creata all'avvio, non al primo upload: meglio
    // scoprire adesso che il percorso non è scrivibile, non alle 2 di notte.
    // `create_dir_all` è idempotente: se esiste già non fa nulla.
    let invoices_dir = config.invoices_dir();
    tokio::fs::create_dir_all(&invoices_dir).await?;
    tracing::info!(dir = %invoices_dir.display(), "storage delle fatture pronto");

    let addr = SocketAddr::from(([127, 0, 0, 1], config.port));

    // `db` e `config` vengono *spostate* dentro lo stato: da qui in poi il
    // proprietario è `state`, e usarle ancora sarebbe un errore di
    // compilazione. È l'ownership di Rust: un valore, un proprietario.
    let state = AppState {
        db,
        config: Arc::new(config),
    };

    let listener = TcpListener::bind(addr).await?;
    tracing::info!("in ascolto su http://{addr}");

    axum::serve(listener, routes::router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

/// Configura i log.
///
/// Il livello si controlla con la variabile `RUST_LOG` (es. `RUST_LOG=debug`);
/// se non c'è usiamo un default sensato: verboso per il nostro crate, sobrio
/// per le dipendenze.
fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "kuba=debug,tower_http=debug".into());

    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .init();
}

/// Attende Ctrl-C e lascia che axum chiuda le richieste in corso prima di
/// spegnersi, invece di troncarle a metà.
///
/// Ci servirà davvero da M3 in poi: quando ci sarà il worker, un'interruzione
/// brutale potrebbe lasciare una fattura bloccata in stato `in_progress`.
async fn shutdown_signal() {
    match tokio::signal::ctrl_c().await {
        Ok(()) => tracing::info!("ricevuto Ctrl-C, spegnimento in corso"),
        Err(err) => tracing::error!(%err, "impossibile ascoltare Ctrl-C"),
    }
}

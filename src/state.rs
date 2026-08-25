//! Lo stato condiviso fra tutti gli handler HTTP.

use std::sync::Arc;

use sqlx::PgPool;

use crate::config::Config;

/// Axum richiede che lo stato sia `Clone`: ne consegna una copia a ogni
/// richiesta, e le richieste sono migliaia. Quindi la copia deve essere
/// economica, mai una copia profonda dei dati.
///
/// - `PgPool` è già un handle condiviso internamente (un `Arc` mascherato):
///   clonarlo incrementa un contatore, non apre nuove connessioni.
/// - `Config` invece è una struct normale con dentro `String` e `PathBuf`:
///   clonarla allocherebbe a ogni richiesta. La avvolgiamo in `Arc`
///   (*Atomically Reference Counted*), che è esattamente lo strumento per dire
///   "un solo dato in memoria, tanti proprietari condivisi, thread-safe".
///   Clonare un `Arc` significa incrementare un contatore atomico; quando
///   l'ultima copia sparisce, il dato viene liberato.
#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub config: Arc<Config>,
}

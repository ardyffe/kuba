//! Lo stato condiviso fra tutti gli handler HTTP.

use sqlx::PgPool;

/// Axum richiede che lo stato sia `Clone`: ne consegna una copia a ogni
/// richiesta, e le richieste sono migliaia. Quindi la copia deve essere
/// economica, mai una copia profonda dei dati.
///
/// `PgPool` va benissimo: è già un handle condiviso internamente (un `Arc`
/// mascherato), quindi clonarlo incrementa un contatore, non apre connessioni.
///
/// Qui dentro finirà anche la `Config` in M1, quando l'handler di upload avrà
/// bisogno di sapere dove salvare i file — e allora servirà avvolgerla in un
/// `Arc`, perché una struct normale con dentro delle `String` verrebbe
/// riallocata a ogni clone. Per ora non c'è: il compilatore ci ha già
/// avvisati con un warning che era un campo mai letto.
#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
}
